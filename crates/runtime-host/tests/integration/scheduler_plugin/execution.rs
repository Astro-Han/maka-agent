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

use super::super::support::{
    client_probe::ClientFixture,
    message_recovery::{ModelRequest, Provider, configure},
    peer::Peer,
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn model_schedules_frozen_root_and_retirement_preserves_accepted_work() {
    tokio::time::timeout(Duration::from_secs(35), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-scheduler-execution-");
    let (provider, mut requests) = Provider::controlled().await;
    let model = configure(&fixture, &provider.base_url).await;
    let mut task_id = String::new();
    let mut run_id = Value::Null;
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("scheduler.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-scheduler-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let guard = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop.clone()),
        );
        if reopened {
            let headless = next(&mut requests).await;
            assert!(headless.body.to_string().contains("HEADLESS_RECOVERY"));
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(300),
                    host.wait_until_idle(Duration::ZERO, Duration::ZERO)
                )
                .await
                .is_err(),
                "zero client connections must not discard an admitted background execution"
            );
            headless.reply.send(answer("HEADLESS_DONE")).unwrap();
        }
        let mut peer = Peer::new(host.clone(), "scheduler-execution").await;
        ready(&mut peer).await;
        if !reopened {
            rpc(&mut peer, "session.create", json!({
                "sessionId":"scheduler-source","workspace":{"kind":"host_path","path":fixture.workspace},
                "modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model},
                "sandboxMode":"workspace-write"
            })).await;
            approve(&mut peer, json!({"kind":"workspace","workspace":{"kind":"host_path","path":fixture.workspace},"sandboxMode":"workspace-write"})).await;
            rpc(&mut peer, "turn.start", json!({
                "sessionId":"scheduler-source","turnId":"schedule-turn","content":{"text":"Schedule independent work"},
                "maxSteps":5
            })).await;
            next(&mut requests)
                .await
                .reply
                .send(tool(
                    "search",
                    "tool_search",
                    json!({"query":"ScheduledTask"}),
                ))
                .unwrap();
            let available = next(&mut requests).await;
            assert!(
                available.body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["function"]["name"] == "ScheduledTask"),
                "{}",
                available.body
            );
            available.reply.send(tool("create", "ScheduledTask", json!({
                "mode":"create","title":"Frozen scheduled work","intentBody":"FROZEN_SCHEDULED_WORK",
                "schedule":{"kind":"once","runAt":jiff::Timestamp::now().as_millisecond()+3_600_000},
                "effect":"agent_run"
            }))).unwrap();
            let listing = next(&mut requests).await;
            let created_task = tool_result(&listing.body);
            task_id = created_task["id"]
                .as_str()
                .expect("scheduled task created")
                .into();
            listing
                .reply
                .send(tool("list", "ScheduledTask", json!({"mode":"list"})))
                .unwrap();
            let final_step = next(&mut requests).await;
            let listed = tool_result(&final_step.body);
            assert_eq!(listed["tasks"][0]["id"], task_id, "{listed}");
            final_step.reply.send(answer("Scheduled.")).unwrap();
            scheduler(
                &mut peer,
                "mutate",
                json!({"kind":"trigger_now","taskId":task_id}),
            )
            .await;
            let background = next(&mut requests).await;
            assert!(
                background
                    .body
                    .to_string()
                    .contains("FROZEN_SCHEDULED_WORK")
            );
            let task = settled(&mut peer, &task_id).await;
            assert_eq!(task["runs"][0]["outcome"], "ok", "{task}");
            let session_id = task["runs"][0]["sessionId"].clone();
            run_id = task["runs"][0]["runId"].clone();
            assert_ne!(session_id, "scheduler-source");
            let session = rpc(
                &mut peer,
                "session.catalog.query",
                json!({"kind":"get","sessionId":session_id}),
            )
            .await;
            assert_eq!(
                session["session"]["sandboxMode"], "workspace-write",
                "{session}"
            );
            rpc(
                &mut peer,
                "plugin.composition.apply",
                json!({"operations":[{
                    "type":"update","entryId":"maka.scheduler","patch":{"disabled":true}
                }]}),
            )
            .await;
            // The accepted model request belongs to Host, not the scheduler Fiber.
            background.reply.send(answer("BACKGROUND_DONE")).unwrap();
            rpc(
                &mut peer,
                "plugin.composition.apply",
                json!({"operations":[{
                    "type":"update","entryId":"maka.scheduler","patch":{"disabled":false}
                }]}),
            )
            .await;
            ready(&mut peer).await;
            approve(
                &mut peer,
                json!({"kind":"session","sessionId":"scheduler-source"}),
            )
            .await;
            let denied = scheduler(&mut peer, "mutate", json!({"kind":"create","input":{
                "title":"Old permission","intentBody":"MUST_NOT_RUN",
                "schedule":{"kind":"once","runAt":jiff::Timestamp::now().as_millisecond()+3_600_000},
                "effect":{"kind":"session_resume","sessionId":"scheduler-source"}
            }})).await;
            let current = rpc(
                &mut peer,
                "session.catalog.query",
                json!({"kind":"get","sessionId":"scheduler-source"}),
            )
            .await;
            let update = rpc(&mut peer, "session.configuration.update", json!({
                "sessionId":"scheduler-source","expectedRevision":current["session"]["revision"],"patch":{"sandboxMode":"danger-full-access"}
            })).await;
            assert_eq!(update["kind"], "committed", "{update}");
            scheduler(
                &mut peer,
                "mutate",
                json!({"kind":"trigger_now","taskId":denied["task"]["id"]}),
            )
            .await;
            let blocked = settled(&mut peer, denied["task"]["id"].as_str().unwrap()).await;
            assert_eq!(blocked["runs"][0]["outcome"], "blocked", "{blocked}");
        } else {
            let task = scheduler(&mut peer, "query", json!({"kind":"get","taskId":task_id})).await;
            assert_eq!(task["task"]["fireCount"], 1);
            assert_eq!(task["task"]["runs"][0]["runId"], run_id);
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(200), requests.recv())
                .await
                .is_err()
        );
        if !reopened {
            rpc(&mut peer, "plugin.composition.apply", json!({"operations":[{
                "type":"update","entryId":"maka.scheduler","patch":{"config":{"timezone":"UTC","misfire":"latest"}}
            }]})).await;
            ready(&mut peer).await;
            scheduler(&mut peer, "mutate", json!({"kind":"create","input":{
                "title":"Restart recovery","intentBody":"HEADLESS_RECOVERY",
                "schedule":{"kind":"once","runAt":jiff::Timestamp::now().as_millisecond()+1000},
                "effect":{"kind":"agent_run","execution":{
                    "cwd":fixture.workspace,"llmConnectionId":model.connection_id,
                    "llmConnectionSlug":model.connection_slug,"model":model.model,
                    "sandboxMode":"workspace-write","approvalPolicy":{"kind":"on-request"},"collaborationMode":"agent","orchestrationMode":"default"
                }}
            }})).await;
        }
        peer.close().await;
        if reopened {
            let mut peer = Peer::new(host.clone(), "background-residency").await;
            approve(&mut peer, json!({"kind":"profile"})).await;
            scheduler(&mut peer, "mutate", json!({"kind":"create","input":{
                "title":"Future work","intentBody":"",
                "schedule":{"kind":"once","runAt":jiff::Timestamp::now().as_millisecond()+3_600_000},
                "effect":{"kind":"notify","channel":"local"}
            }})).await;
            peer.close().await;
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(150),
                    host.wait_until_idle(Duration::ZERO, Duration::ZERO)
                )
                .await
                .is_err(),
                "recoverable scheduled work must retain idle Host"
            );
            let (mut peer, hello) = Peer::handshake(host.clone(), "background-upgrade").await;
            let prepared = rpc(
                &mut peer,
                "host.upgrade.prepare",
                json!({
                    "expectedHostEpoch":hello["hostEpoch"],"allowInterruptActiveTasks":false
                }),
            )
            .await;
            assert_eq!(prepared["kind"], "prepared", "{prepared}");
            peer.close().await;
        }
        stop.cancel();
        server.await.unwrap().unwrap();
        guard.disarm();
        drop(host);
    }
}
async fn rpc(peer: &mut Peer, operation: &str, input: Value) -> Value {
    let response = peer.rpc(operation, input).await;
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}
async fn ready(peer: &mut Peer) {
    loop {
        let status = rpc(peer, "plugin.platform.query", json!({"view":"status"})).await;
        if status["convergence"] == "converged" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn client(peer: &mut Peer) -> Value {
    ready(peer).await;
    let page = rpc(peer, "plugin.client.query", json!({"kind":"snapshot"})).await;
    let entry = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["entryId"] == "maka.scheduler.ui")
        .expect("Scheduler UI active");
    json!({"entryId":entry["entryId"],"extensionId":entry["extensionId"],"activation":entry["activation"],
        "contentDigest":entry["contentDigest"],"clientDigest":entry["clientDigest"]})
}
async fn remote(peer: &mut Peer, input: Value) -> Value {
    let client = client(peer).await;
    let document =
        rpc(peer, "plugin.remote", json!({"kind":"open_document"})).await["document"].clone();
    let binding = json!({"client":client,"method":"request","sessionId":null});
    let target = rpc(
        peer,
        "plugin.remote",
        json!({"kind":"bind","binding":binding}),
    )
    .await["target"]
        .clone();
    let result = rpc(
        peer,
        "plugin.remote",
        json!({"kind":"call","binding":binding,"target":target,"document":document,"input":input}),
    )
    .await;
    rpc(
        peer,
        "plugin.remote",
        json!({"kind":"close_document","document":document}),
    )
    .await;
    result["value"].clone()
}
async fn scheduler(peer: &mut Peer, kind: &str, input: Value) -> Value {
    let request = match kind {
        "query" => json!({"kind":"query","query":input}),
        "mutate" => json!({"kind":"mutate","mutation":input}),
        _ => panic!("invalid Scheduler request"),
    };
    remote(peer, request).await
}
async fn approve(peer: &mut Peer, target: Value) {
    let client = client(peer).await;
    let capabilities = if target["kind"] == "profile" {
        json!(["notifications"])
    } else {
        json!(["executions", "notifications"])
    };
    let grant = rpc(peer, "plugin.authorization", json!({"client":client,"scope":"profile","command":{
        "kind":"approve","request":{"operationId":uuid::Uuid::new_v4(),"title":"Scheduled background work","target":target,"capabilities":capabilities}
    }})).await["grant"].clone();
    remote(peer, json!({"kind":"remember_grant","id":grant["id"]})).await;
}
async fn settled(peer: &mut Peer, id: &str) -> Value {
    loop {
        let result = scheduler(peer, "query", json!({"kind":"get","taskId":id})).await;
        if result["task"]["fireCount"] == 1 {
            return result["task"].clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn next(requests: &mut tokio::sync::mpsc::Receiver<ModelRequest>) -> ModelRequest {
    tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap()
}
fn tool_result(body: &Value) -> Value {
    let result = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .unwrap();
    serde_json::from_str(result["content"].as_str().unwrap())
        .unwrap_or_else(|error| panic!("{error}: {result}"))
}
fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":input.to_string()}}]},"finish_reason":"tool_calls"})
}
fn answer(text: &str) -> Value {
    json!({"index":0,"delta":{"content":text},"finish_reason":"stop"})
}
