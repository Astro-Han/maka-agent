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

use super::support::{
    client_probe::ClientFixture,
    message_recovery::{Provider, configure},
    peer::Peer,
};
use maka_plugins::{
    composition::Scope,
    execution::{Progress, Submit},
    fiber::Fiber,
};
use maka_runtime::event::InvocationOutcome;
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

mod capacity;
mod storage;
mod swarm;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn graph_plugin_recovers_completed_children_after_disable_without_duplicate_work_or_wakes() {
    for implementation in [false, true] {
        tokio::time::timeout(Duration::from_secs(45), scenario(implementation))
            .await
            .unwrap();
    }
}

async fn scenario(implementation: bool) {
    let instruction = "Return CHILD_RESULT exactly.\nKeep this multi-line instruction intact.";
    let full_result = format!(
        "CHILD_RESULT\n{}",
        "多行结果\n\"quoted\"\\path\n".repeat(2000)
    );
    let mut original_result = Value::Null;
    let mut graph_grant = Value::Null;
    let fixture = ClientFixture::new("maka-graph-plugin-");
    if implementation {
        for args in [
            vec!["init", "--quiet"],
            vec![
                "-c",
                "user.name=Maka test",
                "-c",
                "user.email=test@maka.invalid",
                "commit",
                "--allow-empty",
                "--quiet",
                "-m",
                "base",
            ],
        ] {
            let output = std::process::Command::new("git")
                .current_dir(&fixture.workspace)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    let (provider, mut requests) = Provider::controlled().await;
    let model = configure(&fixture, &provider.base_url).await;
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("graph.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-graph-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let guard = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop.clone()),
        );
        let mut peer = Peer::new(host.clone(), "graph-plugin").await;
        ready(&mut peer).await;
        if reopened {
            let tools = peer
                .rpc(
                    "plugin.platform.query",
                    json!({"view":"tools","rootId":"profile"}),
                )
                .await;
            assert_eq!(tools["ok"], true, "{tools}");
            assert!(
                tools["result"]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["toolName"] == "update_agent_graph"),
                "{tools}"
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(250), requests.recv())
                    .await
                    .is_err()
            );
            let driver =
                Fiber::new("driver", "driver", Scope::Session("graph-root".into())).unwrap();
            driver.begin_loading().unwrap();
            let commands = host
                .authorize_plugin_execution(driver.context(), &["graph-root".into()])
                .await
                .unwrap();
            driver.ready().unwrap();
            driver.publish().unwrap();
            commands
                .submit(Submit {
                    operation_id: "next-epoch".into(),
                    orchestration_mode: Some(
                        maka_runtime::execution::BehaviorId::try_from("graph".to_owned()).unwrap(),
                    ),
                    session_id: "graph-root".into(),
                    content: "Start a fresh Graph task; no delegation is necessary.".into(),
                })
                .await
                .unwrap();
            next(&mut requests, "new epoch supervisor")
                .await
                .reply
                .send(tool(
                    "search-new",
                    "tool_search",
                    json!({"query":"agent graph"}),
                ))
                .unwrap();
            next(&mut requests, "new epoch view")
                .await
                .reply
                .send(tool("view-new", "view_agent_graph", json!({})))
                .unwrap();
            let final_answer = next(&mut requests, "new epoch answer").await;
            let result = final_answer.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|message| message["role"] == "tool")
                .unwrap();
            let view: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
            assert_eq!(view["graph"]["closed"], false, "{view}");
            assert_eq!(view["graph"]["work"], json!([]), "{view}");
            assert_eq!(view["graph"]["results"], json!([]), "{view}");
            let directory = remote(&mut peer, "query", json!({"kind":"epochs"})).await;
            let epochs = &directory["result"]["value"]["epochs"];
            assert_eq!(epochs.as_array().unwrap().len(), 2, "{directory}");
            let current = epochs[0]["graphId"].clone();
            let previous = epochs[1]["graphId"].clone();
            let history = remote(
                &mut peer,
                "query",
                json!({"kind":"snapshot","graphId":previous}),
            )
            .await;
            let history = &history["result"]["value"]["graph"];
            assert_eq!(history["finished"], true, "{history}");
            assert_eq!(
                history["selectedResultIds"],
                json!([original_result]),
                "{history}"
            );
            assert_eq!(history["totalWork"], 1, "{history}");
            assert_eq!(
                history["work"][0]["execution"]["resultRecordId"],
                original_result
            );
            assert_eq!(
                history["work"][0]["execution"]["state"], "completed",
                "{history}"
            );
            assert!(
                history["work"][0]["execution"]["sessionId"]
                    .as_str()
                    .is_some()
            );
            let mut watching = Peer::new(host.clone(), "graph-watcher").await;
            let detail = remote(
                &mut peer,
                "query",
                json!({"kind":"work","graphId":previous,
                "workId":history["work"][0]["workId"],"offset":0}),
            )
            .await;
            assert_eq!(
                detail["result"]["value"]["work"]["instruction"], instruction,
                "{detail}"
            );
            assert!(detail["result"]["value"]["work"]["nextOffset"].is_null());
            let mut offset = 0;
            let mut restored = String::new();
            loop {
                let response = remote(&mut peer, "query", json!({"kind":"result","graphId":previous,
                    "workId":history["work"][0]["workId"],"recordId":original_result,"offset":offset})).await;
                assert_eq!(response["ok"], true, "{response}");
                let page = &response["result"]["value"]["result"];
                assert_eq!(page["offset"], offset, "{response}");
                assert_eq!(page["totalBytes"], full_result.len());
                restored.push_str(page["text"].as_str().unwrap());
                let Some(next) = page["nextOffset"].as_u64() else {
                    break;
                };
                assert!(next > offset);
                offset = next;
            }
            assert_eq!(restored, full_result);
            let (binding, target, document) = bind_remote(&mut watching, "changes").await;
            let opened = watching
                .rpc(
                    "plugin.remote",
                    json!({"kind":"open","binding":binding,
                "target":target,"document":document,"input":null}),
                )
                .await;
            assert_eq!(opened["ok"], true, "{opened}");
            let stream = opened["result"]["stream"].clone();
            let first = watching
                .rpc(
                    "plugin.remote",
                    json!({"kind":"next","document":document,"stream":stream}),
                )
                .await;
            assert_eq!(first["result"]["kind"], "item", "{first}");
            watching.send_rpc(
                "graph-idle",
                "plugin.remote",
                json!({"kind":"next","document":document,"stream":stream}),
            );
            let unrelated = peer.rpc("session.create", json!({
                "sessionId":"unrelated", "workspace":{"kind":"host_path","path":fixture.workspace},
                "modelTarget":{"kind":"explicit","connectionId":model.connection_id,
                    "connectionSlug":model.connection_slug,"model":model.model}
            })).await;
            assert_eq!(unrelated["ok"], true, "{unrelated}");
            assert!(
                tokio::time::timeout(Duration::from_millis(300), async {
                    loop {
                        let event = watching.frame().await;
                        if event["requestId"] == "graph-idle" {
                            return event;
                        }
                    }
                })
                .await
                .is_err(),
                "unrelated commits must not refresh Graph history"
            );
            let (grant_binding, _, grant_document) = bind_remote(&mut peer, "authorize").await;
            let revoked = peer
                .rpc(
                    "plugin.authorization",
                    json!({
                        "client":grant_binding["client"], "scope":"profile",
                        "command":{"kind":"revoke", "id":graph_grant}
                    }),
                )
                .await;
            assert_eq!(revoked["ok"], true, "{revoked}");
            let closed = peer
                .rpc(
                    "plugin.remote",
                    json!({"kind":"close_document","document":grant_document}),
                )
                .await;
            assert_eq!(closed["ok"], true, "{closed}");
            let stale = remote(&mut peer, "stop", json!({"graphId":previous})).await;
            assert_eq!(stale["ok"], false, "{stale}");
            assert!(matches!(
                commands.query("next-epoch".into()).await.unwrap().progress,
                Progress::Running
            ));
            let stopped = remote(&mut peer, "stop", json!({"graphId":current})).await;
            assert_eq!(stopped["ok"], true, "{stopped}");
            loop {
                let event = watching.frame().await;
                if event["requestId"] == "graph-idle" {
                    assert_eq!(event["result"]["kind"], "item", "{event}");
                    break;
                }
            }
            watching.close().await;
            loop {
                if let Progress::Ended { outcome } =
                    commands.query("next-epoch".into()).await.unwrap().progress
                {
                    assert!(
                        matches!(outcome, InvocationOutcome::Cancelled { .. }),
                        "{outcome:?}"
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            drop(final_answer);
            driver
                .shutdown(tokio::time::Instant::now() + Duration::from_secs(2))
                .await
                .unwrap();
        } else {
            presets(&mut peer, json!([{"id":"reader","name":"Review worker","description":"Read-only review",
                "profile":if implementation { "implementation" } else { "local_read" },"connectionSlug":model.connection_slug,"model":model.model,"enabled":true}])).await;
            let created = peer.rpc("session.create", json!({
                "sessionId":"graph-root", "workspace":{"kind":"host_path","path":fixture.workspace},
                "modelTarget":{"kind":"explicit","connectionId":model.connection_id,
                    "connectionSlug":model.connection_slug,"model":model.model},
                "orchestrationMode":"default"
            })).await;
            assert_eq!(created["ok"], true, "{created}");
            graph_grant = approve(&mut peer, "graph-root").await;
            let driver =
                Fiber::new("driver", "driver", Scope::Session("graph-root".into())).unwrap();
            driver.begin_loading().unwrap();
            let commands = host
                .authorize_plugin_execution(driver.context(), &["graph-root".into()])
                .await
                .unwrap();
            driver.ready().unwrap();
            driver.publish().unwrap();
            commands
                .submit(Submit {
                    operation_id: "initial".into(),
                    orchestration_mode: Some(
                        maka_runtime::execution::BehaviorId::try_from("graph".to_owned()).unwrap(),
                    ),
                    session_id: "graph-root".into(),
                    content: "Coordinate a Graph acceptance task".into(),
                })
                .await
                .unwrap();
            let first = next(&mut requests, "initial supervisor").await;
            assert!(
                first.body.to_string().contains("Agent Graph supervisor"),
                "{}",
                first.body
            );
            assert!(
                first.body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["function"]["name"] == "Read")
            );
            first
                .reply
                .send(tool(
                    "search",
                    "tool_search",
                    json!({"query":"agent graph"}),
                ))
                .unwrap();
            let second = next(&mut requests, "schedule decision").await;
            assert!(
                second.body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["function"]["name"] == "update_agent_graph"),
                "{}",
                second.body
            );
            second
                .reply
                .send(tool("list", "agent_list", json!({})))
                .unwrap();
            let scheduling = next(&mut requests, "available preset catalog").await;
            assert!(scheduling.body.to_string().contains("Review worker"));
            scheduling.reply.send(tool("schedule", "update_agent_graph", json!({
                "operation":"add_work","work":[{"target":{"kind":"preset","presetId":"reader"},"instruction":instruction}]
            }))).unwrap();
            let mut child = None;
            let mut yielded = false;
            while child.is_none() || !yielded {
                let request = next(&mut requests, "child or yield").await;
                if request.body.to_string().contains("Agent Graph supervisor") {
                    assert!(
                        !yielded,
                        "supervisor made an extra model step: {}",
                        request.body
                    );
                    assert!(
                        request.body.to_string().contains("committed"),
                        "{}",
                        request.body
                    );
                    request
                        .reply
                        .send(tool("yield", "yield_agent_graph", json!({})))
                        .unwrap();
                    yielded = true;
                } else {
                    assert!(request.body.to_string().contains("CHILD_RESULT"));
                    assert!(request.body.to_string().contains(if implementation {
                        "dedicated worktree"
                    } else {
                        "local-read child agent"
                    }));
                    assert!(
                        request.body["tools"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|tool| {
                                if implementation {
                                    matches!(
                                        tool["function"]["name"].as_str(),
                                        Some(
                                            "Read"
                                                | "Glob"
                                                | "Grep"
                                                | "Write"
                                                | "Edit"
                                                | "apply_patch"
                                                | "shell"
                                                | "WriteStdin"
                                                | "StopBackgroundTask"
                                                | "tool_search"
                                        )
                                    )
                                } else {
                                    matches!(
                                        tool["function"]["name"].as_str(),
                                        Some("Read" | "Glob" | "Grep" | "tool_search")
                                    )
                                }
                            })
                    );
                    child = Some(request);
                }
            }
            loop {
                if let Progress::Ended { outcome } =
                    commands.query("initial".into()).await.unwrap().progress
                {
                    assert_eq!(outcome, InvocationOutcome::Completed);
                    break;
                }
                tokio::select! {
                    extra = requests.recv() => panic!("unexpected model request after yield: {}", extra.unwrap().body),
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                }
            }
            // Recovery uses the reserved definition even after its preset is removed.
            presets(&mut peer, json!([])).await;
            toggle(&mut peer, true).await;
            ready(&mut peer).await;
            let mut child = child.unwrap();
            if implementation {
                child
                    .reply
                    .send(tool("find-write", "tool_search", json!({"query":"Write"})))
                    .unwrap();
                child = next(&mut requests, "implementation write").await;
                child
                    .reply
                    .send(tool(
                        "write",
                        "Write",
                        json!({"path":"result.txt","content":"isolated change\n"}),
                    ))
                    .unwrap();
                child = next(&mut requests, "implementation completion").await;
                assert!(child.body.to_string().contains("result.txt"));
                assert!(!fixture.workspace.join("result.txt").exists());
            }
            child.reply.send(answer(&full_result)).unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(250), requests.recv())
                    .await
                    .is_err(),
                "disabled Graph must not submit a supervisor wake"
            );
            toggle(&mut peer, false).await;
            ready(&mut peer).await;
            let wake = next(&mut requests, "reactivated supervisor wake").await;
            let text = wake.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|message| message["content"].as_str())
                .find(|text| text.starts_with("Agent Graph has new durable outcomes."))
                .unwrap_or_else(|| panic!("{}", wake.body));
            let snapshot: Value = serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
            assert!(
                snapshot["results"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("CHILD_RESULT"),
                "{snapshot}"
            );
            let result_id = snapshot["results"][0]["recordId"].clone();
            if implementation {
                assert!(
                    snapshot["results"][0]["workspacePatch"]["bytes"]
                        .as_u64()
                        .unwrap()
                        > 0,
                    "{snapshot}"
                );
            }
            original_result = result_id.clone();
            assert_eq!(snapshot["results"][0]["truncated"], true);
            wake.reply
                .send(tool(
                    "search-finish",
                    "tool_search",
                    json!({"query":"agent graph"}),
                ))
                .unwrap();
            let finishing = next(&mut requests, "finish decision").await;
            finishing
                .reply
                .send(tool(
                    "read-result",
                    "view_agent_graph",
                    json!({
                        "record_id":result_id,"work_id":snapshot["results"][0]["workId"],"offset":0
                    }),
                ))
                .unwrap();
            let mut selecting = next(&mut requests, "full result page").await;
            let page = selecting.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|message| message["role"] == "tool")
                .unwrap();
            let page: Value = serde_json::from_str(page["content"].as_str().unwrap()).unwrap();
            assert_eq!(page["result"]["totalBytes"], full_result.len(), "{page}");
            assert!(full_result.starts_with(page["result"]["text"].as_str().unwrap()));
            assert!(page["result"]["nextOffset"].as_u64().unwrap() > 0);
            assert_eq!(page["result"]["isolatedWorkspace"], implementation);
            if implementation {
                selecting.reply.send(tool("read-patch", "view_agent_graph", json!({
                    "record_id":result_id,"work_id":snapshot["results"][0]["workId"],"offset":0,"part":"patch"
                }))).unwrap();
                selecting = next(&mut requests, "workspace patch page").await;
                let patch = selecting.body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|message| message["role"] == "tool")
                    .unwrap();
                let patch: Value =
                    serde_json::from_str(patch["content"].as_str().unwrap()).unwrap();
                assert_eq!(patch["result"]["part"], "patch");
                assert!(
                    patch["result"]["text"]
                        .as_str()
                        .unwrap()
                        .contains("+isolated change"),
                    "{patch}"
                );
            }
            selecting.reply.send(tool("finish", "update_agent_graph", json!({
                "operation":"finish","result_ids":[result_id],"reason":"The child completed the requested task."
            }))).unwrap();
            let final_answer = next(&mut requests, "final answer").await;
            assert!(
                final_answer.body.to_string().contains("committed"),
                "{}",
                final_answer.body
            );
            final_answer.reply.send(answer("Graph completed.")).unwrap();
            driver
                .shutdown(tokio::time::Instant::now() + Duration::from_secs(2))
                .await
                .unwrap();
        }
        peer.close().await;
        tokio::time::timeout(
            Duration::from_secs(10),
            host.wait_until_idle(Duration::from_secs(1), Duration::from_millis(100)),
        )
        .await
        .unwrap();
        stop.cancel();
        server.await.unwrap().unwrap();
        guard.disarm();
        drop(host);
    }
    {
        let recorded = provider.requests.lock().unwrap();
        assert_eq!(
            recorded
                .iter()
                .filter(|body| !body.to_string().contains("Agent Graph supervisor"))
                .count(),
            if implementation { 3 } else { 1 }
        );
        assert_eq!(recorded.len(), if implementation { 15 } else { 12 });
    }
    let log = Arc::new(
        maka_event_log::EventLog::for_root(Arc::new(fixture.owner()))
            .await
            .unwrap(),
    );
    let repository = storage::repository(log.clone());
    use maka_graph::store::Store as _;
    let control = repository.current("graph-root").await.unwrap().unwrap();
    assert!(control.stop_requested);
    assert_eq!(control.epoch.epoch, 2);
    let epochs = repository.epochs("graph-root", Some(2)).await.unwrap();
    let previous = repository
        .control("graph-root", &epochs.epochs[0].graph_id)
        .await
        .unwrap();
    assert!(previous.finished);
    drop(repository);
    Arc::try_unwrap(log).ok().unwrap().close().await.unwrap();
}

async fn presets(peer: &mut Peer, presets: Value) {
    let current = remote(peer, "settings", json!({"kind":"read"})).await;
    assert_eq!(current["ok"], true, "{current}");
    let changed = remote(
        peer,
        "settings",
        json!({
            "kind":"replace",
            "snapshot":{"revision":current["result"]["value"]["revision"],"presets":presets}
        }),
    )
    .await;
    assert_eq!(changed["ok"], true, "{changed}");
    let stale = remote(
        peer,
        "settings",
        json!({
            "kind":"replace",
            "snapshot":{"revision":current["result"]["value"]["revision"],"presets":[]}
        }),
    )
    .await;
    assert_eq!(
        stale["ok"], false,
        "stale preset writes must not overwrite newer data"
    );
}

// Every call crosses the same document, identity and registration checks as a Client SDK handle.
async fn bind_remote(peer: &mut Peer, method: &str) -> (Value, Value, Value) {
    bind_remote_session(
        peer,
        method,
        if method == "settings" {
            None
        } else {
            Some("graph-root")
        },
    )
    .await
}
async fn bind_remote_session(
    peer: &mut Peer,
    method: &str,
    session: Option<&str>,
) -> (Value, Value, Value) {
    let catalog = peer
        .rpc("plugin.client.query", json!({"kind":"snapshot"}))
        .await;
    assert_eq!(catalog["ok"], true, "{catalog}");
    let entry = catalog["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == "maka.agent-graph")
        .unwrap();
    let client = json!({"entryId":entry["entryId"],"extensionId":entry["extensionId"],
        "activation":entry["activation"],"contentDigest":entry["contentDigest"],"clientDigest":entry["clientDigest"]});
    let binding = json!({"client":client,"method":method,"sessionId":session});
    let bound = peer
        .rpc("plugin.remote", json!({"kind":"bind","binding":binding}))
        .await;
    assert_eq!(bound["ok"], true, "{bound}");
    let opened = peer
        .rpc("plugin.remote", json!({"kind":"open_document"}))
        .await;
    assert_eq!(opened["ok"], true, "{opened}");
    let document = opened["result"]["document"].clone();
    (binding, bound["result"]["target"].clone(), document)
}

async fn remote(peer: &mut Peer, method: &str, input: Value) -> Value {
    let (binding, target, document) = bind_remote(peer, method).await;
    let reply = peer
        .rpc(
            "plugin.remote",
            json!({"kind":"call","document":document,
        "binding":binding,"target":target,"input":input}),
        )
        .await;
    let closed = peer
        .rpc(
            "plugin.remote",
            json!({"kind":"close_document","document":document}),
        )
        .await;
    assert_eq!(closed["ok"], true, "{closed}");
    reply
}

async fn ready(peer: &mut Peer) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let status = peer
            .rpc("plugin.platform.query", json!({"view":"status"}))
            .await;
        assert_eq!(status["ok"], true, "{status}");
        assert!(
            tokio::time::Instant::now() < deadline,
            "platform did not converge: {status}"
        );
        if status["result"]["convergence"] == "converged" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn next(
    requests: &mut tokio::sync::mpsc::Receiver<super::support::message_recovery::ModelRequest>,
    stage: &str,
) -> super::support::message_recovery::ModelRequest {
    tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap_or_else(|_| panic!("missing model request: {stage}"))
        .expect("provider request channel")
}
async fn toggle(peer: &mut Peer, disabled: bool) {
    let result = peer.rpc("plugin.composition.apply", json!({
        "operations":[{"type":"update","entryId":"maka.agent-graph","patch":{"disabled":disabled}}]
    })).await;
    assert_eq!(result["ok"], true, "{result}");
}
fn tool(id: &str, name: &str, arguments: Value) -> Value {
    json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"})
}
fn answer(text: &str) -> Value {
    json!({"index":0,"delta":{"content":text},"finish_reason":"stop"})
}

async fn approve(peer: &mut Peer, session: &str) -> Value {
    let (binding, target, document) = bind_remote_session(peer, "authorize", Some(session)).await;
    let consent = peer.rpc("plugin.authorization", json!({
        "client": binding["client"], "scope": "profile", "command": {
            "kind": "approve", "request": {
                "operationId": uuid::Uuid::new_v4(), "title": "Agent Graph background execution",
                "target": {"kind":"session", "sessionId":session}, "capabilities": ["executions"]
            }
        }
    })).await;
    assert_eq!(consent["ok"], true, "{consent}");
    let reply = peer
        .rpc(
            "plugin.remote",
            json!({"kind":"call", "binding":binding,
        "target":target, "document":document,
        "input":{"kind":"remember","id":consent["result"]["grant"]["id"]}}),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let closed = peer
        .rpc(
            "plugin.remote",
            json!({"kind":"close_document","document":document}),
        )
        .await;
    assert_eq!(closed["ok"], true, "{closed}");
    consent["result"]["grant"]["id"].clone()
}
