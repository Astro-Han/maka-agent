/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn swarm_handles_small_tasks_directly_and_wakes_only_after_the_batch_settles() {
    tokio::time::timeout(Duration::from_secs(30), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-swarm-");
    let (provider, mut requests) = Provider::controlled().await;
    let model = configure(&fixture, &provider.base_url).await;
    let host = Host::open(fixture.owner()).await.unwrap();
    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("swarm.sock");
    #[cfg(windows)]
    let endpoint =
        std::path::PathBuf::from(format!(r"\\.\pipe\maka-swarm-{}", uuid::Uuid::new_v4()));
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop.clone()),
    );
    let mut peer = Peer::new(host.clone(), "swarm").await;
    ready(&mut peer).await;
    presets(&mut peer,json!([{"id":"reader","name":"Reader","description":"Read-only worker",
        "profile":"local_read","connectionSlug":model.connection_slug,"model":model.model,"enabled":true}])).await;
    assert_eq!(peer.rpc("session.create",json!({
        "sessionId":"graph-root","workspace":{"kind":"host_path","path":fixture.workspace},
        "modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model},
        "orchestrationMode":"default"
    })).await["ok"],true);
    assert_eq!(
        peer.rpc(
            "turn.start",
            json!({"sessionId":"graph-root","turnId":"small","content":{"text":"Small task"},"turnOrchestration":{"mode":"swarm","source":"slash_command"}})
        )
        .await["ok"],
        true
    );
    let small = next(&mut requests, "small direct task").await;
    assert!(small.body.to_string().contains("handle small"));
    assert!(
        small.body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "Read")
    );
    small
        .reply
        .send(tool(
            "resume-search",
            "tool_search",
            json!({"query":"view_agent_graph"}),
        ))
        .unwrap();
    next(&mut requests, "resume Graph tool")
        .await
        .reply
        .send(tool("resume-view", "view_agent_graph", json!({})))
        .unwrap();
    let interrupted = next(&mut requests, "after Graph tool").await;
    let running = peer
        .rpc(
            "turn.query",
            json!({"sessionId":"graph-root","turnId":"small"}),
        )
        .await;
    let stopped = peer
        .rpc(
            "turn.stop",
            json!({
                "sessionId":"graph-root","turnId":"small","runId":running["result"]["runId"]
            }),
        )
        .await;
    assert_eq!(stopped["ok"], true, "{stopped}");
    loop {
        let state = peer
            .rpc(
                "turn.query",
                json!({"sessionId":"graph-root","turnId":"small"}),
            )
            .await;
        if state["result"]["status"] == "cancelled" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    drop(interrupted);
    // Resume uses the sealed invocation mode, even when the current Session
    // default changes. Query must not reject an orchestration Session up front.
    mode(&mut peer, "graph").await;
    let resume = peer
        .rpc("turn.resume.query", json!({"sessionId":"graph-root"}))
        .await;
    assert_eq!(resume["result"]["disposition"], "ready", "{resume}");
    let started = peer
        .rpc(
            "turn.resume.start",
            json!({
                "sessionId":"graph-root", "turnId":"resume-small",
                "sourceRunId":resume["result"]["sourceRunId"],
                "sourceRuntimeEventHighWater":resume["result"]["sourceRuntimeEventHighWater"]
            }),
        )
        .await;
    assert_eq!(started["result"]["kind"], "started", "{started}");
    let resumed = next(&mut requests, "frozen Swarm resume").await;
    assert!(
        resumed.body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Agent Swarm supervisor")
    );
    resumed.reply.send(answer("resumed direct answer")).unwrap();
    completed(&mut peer, "resume-small").await;
    mode(&mut peer, "default").await;
    let conflict = peer.rpc("turn.start",json!({"sessionId":"graph-root","turnId":"conflicting-mode",
        "content":{"text":"Do not destroy the current Swarm"},"turnOrchestration":{"mode":"graph","source":"slash_command"}})).await;
    assert_eq!(conflict["ok"], false, "{conflict}");
    assert_eq!(peer.rpc("turn.start",json!({"sessionId":"graph-root","turnId":"batch","content":{"text":"Delegate two independent inspections"},"turnOrchestration":{"mode":"swarm","source":"slash_command"}})).await["ok"],true);
    next(&mut requests, "discover")
        .await
        .reply
        .send(tool(
            "search",
            "tool_search",
            json!({"query":"agent graph swarm"}),
        ))
        .unwrap();
    next(&mut requests,"schedule").await.reply.send(tool("schedule","update_agent_graph",json!({
        "operation":"add_work","work":[
            {"target":{"kind":"preset","presetId":"reader"},"instruction":"Inspect SWARM_A"},
            {"target":{"kind":"preset","presetId":"reader"},"instruction":"Inspect SWARM_B"}
        ]
    }))).unwrap();
    let mut children = Vec::new();
    let mut yielded = false;
    while children.len() != 2 || !yielded {
        let request = next(&mut requests, "children and yield").await;
        if request.body.to_string().contains("Agent Swarm supervisor") {
            assert!(!yielded);
            request
                .reply
                .send(tool("yield", "yield_agent_graph", json!({})))
                .unwrap();
            yielded = true;
        } else {
            children.push(request);
        }
    }
    completed(&mut peer, "batch").await;
    children
        .pop()
        .unwrap()
        .reply
        .send(answer("PRIVATE_FINAL_A"))
        .unwrap();
    // Observe committed completion through the read-only UI path before checking
    // that reconciliation does not emit a premature supervisor request.
    let directory = remote(&mut peer, "query", json!({"kind":"epochs"})).await;
    let graph = directory["result"]["value"]["epochs"][0]["graphId"].clone();
    loop {
        let snapshot = remote(
            &mut peer,
            "query",
            json!({"kind":"snapshot","graphId":graph}),
        )
        .await;
        if snapshot["result"]["value"]["graph"]["work"]
            .as_array()
            .unwrap()
            .iter()
            .any(|work| work["execution"]["state"] == "completed")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(350), requests.recv())
            .await
            .is_err(),
        "one completed sibling is not a Swarm checkpoint"
    );
    children
        .pop()
        .unwrap()
        .reply
        .send(answer("PRIVATE_FINAL_B"))
        .unwrap();
    let wake = next(&mut requests, "batch checkpoint").await;
    assert!(
        wake.body
            .to_string()
            .contains("Agent Swarm reached a checkpoint")
    );
    assert!(
        !wake.body.to_string().contains("PRIVATE_FINAL"),
        "wake leaked result bodies"
    );
    wake.reply
        .send(tool(
            "search-status",
            "tool_search",
            json!({"query":"agent_swarm_status view_agent_graph update_agent_graph"}),
        ))
        .unwrap();
    next(&mut requests, "status")
        .await
        .reply
        .send(tool("status", "agent_swarm_status", json!({})))
        .unwrap();
    let status = next(&mut requests, "status-only result").await;
    let compact = last_tool(&status.body);
    assert_eq!(compact["status"], "settled", "{compact}");
    assert_eq!(compact["counts"]["completed"], 2, "{compact}");
    assert_eq!(compact["items"].as_array().unwrap().len(), 2);
    assert!(!compact.to_string().contains("PRIVATE_FINAL"));
    let selected = compact["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["resultRecordId"].clone())
        .collect::<Vec<_>>();
    status
        .reply
        .send(tool("epochs", "view_agent_graph", json!({"kind":"epochs"})))
        .unwrap();
    let epochs = next(&mut requests, "enumerate graph history").await;
    assert_eq!(last_tool(&epochs.body)["epochs"][0]["graphId"], graph);
    epochs
        .reply
        .send(tool(
            "page",
            "view_agent_graph",
            json!({"kind":"snapshot","graphId":graph}),
        ))
        .unwrap();
    let page = next(&mut requests, "enumerate final IDs").await;
    let work = last_tool(&page.body)["graph"]["work"].clone();
    assert_eq!(work.as_array().unwrap().len(), 2);
    for item in work.as_array().unwrap() {
        assert!(selected.contains(&item["execution"]["resultRecordId"]));
    }
    page.reply
        .send(tool(
            "finish",
            "update_agent_graph",
            json!({"operation":"finish","result_ids":selected,"reason":"All useful work settled"}),
        ))
        .unwrap();
    next(&mut requests, "final answer")
        .await
        .reply
        .send(answer("synthesized"))
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), requests.recv())
            .await
            .is_err(),
        "settled checkpoint replayed"
    );
    loop {
        let started = peer.rpc("turn.start",json!({"sessionId":"graph-root","turnId":"plain","content":{"text":"Ordinary task"}})).await;
        if started["ok"] == true {
            break;
        }
        assert_eq!(started["error"]["code"], "session_busy", "{started}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let plain = next(&mut requests, "Session default after one-off Swarm").await;
    assert!(
        !plain.body["messages"][0]
            .to_string()
            .contains("Agent Swarm supervisor")
    );
    plain.reply.send(answer("ordinary answer")).unwrap();
    completed(&mut peer, "plain").await;
    peer.close().await;
    stop.cancel();
    server.await.unwrap().unwrap();
    cleanup.disarm();
    drop(host);
    let log = fixture.log().await;
    let control = log
        .graph_control("graph-root", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(control.epoch.mode, maka_graph::Mode::Swarm);
    assert!(control.finished);
    assert_eq!(
        log.get_session::<maka_runtime_host::session::SessionConfiguration>("graph-root")
            .await
            .unwrap()
            .unwrap()
            .configuration
            .orchestration_mode,
        maka_runtime::execution::OrchestrationMode::Default
    );
    let prefix = log.prefix(500, 4 * 1024 * 1024).await.unwrap();
    for row in &prefix.events {
        if row.event.invocation.session_id == "graph-root"
            && let maka_runtime::event::Fact::InvocationOpened {
                configuration: Some(configuration),
                ..
            } = &row.event.fact
        {
            assert_eq!(
                configuration.orchestration_mode,
                if row.event.invocation.turn_id == "plain" {
                    maka_runtime::execution::OrchestrationMode::Default
                } else {
                    maka_runtime::execution::OrchestrationMode::Swarm
                }
            );
        }
    }
    log.close().await.unwrap();
}
async fn completed(peer: &mut Peer, turn: &str) {
    loop {
        let state = peer
            .rpc(
                "turn.query",
                json!({"sessionId":"graph-root","turnId":turn}),
            )
            .await;
        if !matches!(
            state["result"]["status"].as_str(),
            Some("admitted" | "created" | "running")
        ) {
            assert_eq!(state["result"]["status"], "completed", "{state}");
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn mode(peer: &mut Peer, mode: &str) {
    let session = peer
        .rpc(
            "session.catalog.query",
            json!({"kind":"get","sessionId":"graph-root"}),
        )
        .await;
    let updated = peer.rpc("session.configuration.update", json!({
        "sessionId":"graph-root", "expectedRevision":session["result"]["session"]["revision"],
        "patch":{"orchestrationMode":mode}
    })).await;
    assert_eq!(updated["ok"], true, "{updated}");
}
fn last_tool(body: &Value) -> Value {
    let message = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .unwrap();
    serde_json::from_str(message["content"].as_str().unwrap()).unwrap()
}
