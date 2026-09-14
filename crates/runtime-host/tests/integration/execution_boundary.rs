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
    message_recovery::{ModelRequest, Provider, configure},
    peer::Peer,
};
use maka_protocol::Operation;
use maka_runtime::{event::Fact, execution::PermissionMode};
use maka_runtime_host::{
    server::{Host, local::LocalListener},
    session::SessionConfiguration,
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const SESSION: &str = "live-boundary";
const MARKER: &str = "boundary-granted-source";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_grants_preserve_calls_and_narrowing_drains_resources_before_reopen() {
    tokio::time::timeout(Duration::from_secs(30), scenario())
        .await
        .unwrap();
}

async fn scenario() {
    let fixture = ClientFixture::new("maka-boundary-");
    let outside = fixture.workspace.parent().unwrap().join("outside.txt");
    std::fs::write(&outside, MARKER).unwrap();
    let (provider, mut requests) = Provider::controlled().await;
    let model = configure(&fixture, &provider.base_url).await;
    let mut persisted = Value::Null;
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("boundary.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-boundary-{}", uuid::Uuid::new_v4()));
        let cancel = CancellationToken::new();
        let cleanup = cancel.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), cancel.clone()),
        );
        let mut peer = Peer::new(host.clone(), "boundary-client").await;
        if !reopened {
            let created = peer
                .rpc(
                    Operation::SessionCreate.as_str(),
                    json!({
                        "sessionId":SESSION, "mode":"deep_research",
                        "workspace":{"kind":"host_path","path":fixture.workspace},
                        "modelTarget":{"kind":"explicit","connectionId":model.connection_id,
                            "connectionSlug":model.connection_slug,"model":model.model}
                    }),
                )
                .await;
            assert_eq!(created["ok"], true, "{created}");
            assert_eq!(created["result"]["permissionMode"], "explore");
            start(&mut peer, "read").await;
            let first = request(&mut requests).await;
            first
                .reply
                .send(call("denied", "Read", json!({"path":outside})))
                .unwrap();
            let second = request(&mut requests).await;
            let denied = tool_messages(&second.body);
            assert_eq!(denied.len(), 1, "{denied:?}");
            assert!(!denied[0].contains(MARKER));
            assert!(
                denied[0].contains("outside the admitted Read roots"),
                "{denied:?}"
            );
            let old = revision(&mut peer).await;
            let mixed = update(
                &mut peer,
                old,
                json!({"permissionMode":"bypass","orchestrationMode":"default"}),
            )
            .await;
            assert_eq!(mixed["error"]["code"], "session_busy", "{mixed}");
            let granted = update(&mut peer, old, json!({"permissionMode":"bypass"})).await;
            assert_eq!(granted["result"]["kind"], "committed", "{granted}");
            assert_eq!(
                boundary(&mut peer).await,
                json!({"kind":"bypass","revision":1})
            );
            let stale = update(&mut peer, old, json!({"permissionMode":"ask"})).await;
            assert_eq!(stale["result"]["kind"], "revision_conflict", "{stale}");
            let current = revision(&mut peer).await;
            let narrowed = update(&mut peer, current, json!({"permissionMode":"ask"})).await;
            assert_eq!(narrowed["error"]["code"], "session_busy", "{narrowed}");
            second
                .reply
                .send(call("granted", "Read", json!({"path":outside})))
                .unwrap();
            let third = request(&mut requests).await;
            let messages = tool_messages(&third.body);
            assert_eq!(messages.len(), 2, "{messages:?}");
            assert!(messages[1].contains(MARKER), "{messages:?}");
            third.reply.send(done()).unwrap();
            finish(&mut peer, "read").await;

            // A fresh activation sees the wider tool set. Its background process
            // outlives the Turn and must be gone before idle narrowing commits.
            start(&mut peer, "shell").await;
            let shell = request(&mut requests).await;
            #[cfg(unix)]
            let command = "exec sleep 60";
            #[cfg(windows)]
            let command = "Start-Sleep -Seconds 60";
            shell
                .reply
                .send(call(
                    "background",
                    maka_process::SHELL_NAME,
                    json!({"command":command,"run_in_background":true}),
                ))
                .unwrap();
            let result = request(&mut requests).await;
            assert!(
                tool_messages(&result.body)
                    .last()
                    .unwrap()
                    .contains("background-tasks"),
                "{}",
                result.body
            );
            result.reply.send(done()).unwrap();
            finish(&mut peer, "shell").await;
            let active = resources(&mut peer).await;
            assert_eq!(active.len(), 1);
            assert!(
                matches!(
                    active[0]["result"]["status"].as_str(),
                    Some("starting" | "running")
                ),
                "{active:?}"
            );
            let revision = revision(&mut peer).await;
            let narrowed = update(&mut peer, revision, json!({"permissionMode":"ask"})).await;
            assert_eq!(narrowed["result"]["kind"], "committed", "{narrowed}");
            assert_eq!(
                boundary(&mut peer).await,
                json!({"kind":"managed","access":"writable","revision":2})
            );
            let resources = resources(&mut peer).await;
            assert_eq!(
                resources[0]["result"]["status"], "cancelled",
                "{resources:?}"
            );
            persisted = resources[0].clone();
        } else {
            assert_eq!(
                boundary(&mut peer).await,
                json!({"kind":"managed","access":"writable","revision":2})
            );
            assert_eq!(resources(&mut peer).await, vec![persisted.clone()]);
        }
        peer.close().await;
        drop(cleanup);
        server.await.unwrap().unwrap();
        drop(host);
    }
    let log = fixture.log().await;
    let config = log
        .get_session::<SessionConfiguration>(SESSION)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(config.configuration.permission_mode, PermissionMode::Ask);
    assert_eq!(config.configuration.boundary_revision, 2);
    assert!(
        !config
            .configuration
            .labels
            .iter()
            .any(|label| label == "mode:deep_research")
    );
    let prefix = log.prefix(200, 1024 * 1024).await.unwrap();
    let openings: Vec<_> = prefix
        .events
        .iter()
        .filter_map(|event| match &event.event.fact {
            Fact::InvocationOpened {
                configuration: Some(config),
                ..
            } => Some(config.permission_mode),
            _ => None,
        })
        .collect();
    assert_eq!(
        openings,
        vec![PermissionMode::Explore, PermissionMode::Bypass]
    );
    log.close().await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 5);
}

async fn request(requests: &mut mpsc::Receiver<ModelRequest>) -> ModelRequest {
    tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap()
}

fn call(id: &str, name: &str, input: Value) -> Value {
    json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function",
        "function":{"name":name,"arguments":input.to_string()}}]},"finish_reason":"tool_calls"})
}

fn done() -> Value {
    json!({"index":0,"delta":{"content":"done"},"finish_reason":"stop"})
}

fn tool_messages(request: &Value) -> Vec<String> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].as_str().unwrap().to_owned())
        .collect()
}

async fn start(peer: &mut Peer, turn: &str) {
    let result = peer
        .rpc(
            Operation::TurnStart.as_str(),
            json!({"sessionId":SESSION,"turnId":turn,
        "content":{"text":"Read the fixture and complete the task."},"maxSteps":4}),
        )
        .await;
    assert_eq!(result["ok"], true, "{result}");
}

async fn finish(peer: &mut Peer, turn: &str) {
    loop {
        let result = peer
            .rpc(
                Operation::TurnQuery.as_str(),
                json!({"sessionId":SESSION,"turnId":turn}),
            )
            .await;
        match result["result"]["status"].as_str() {
            Some("completed") => return,
            Some("failed" | "cancelled") => panic!("{result}"),
            _ => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
}

async fn revision(peer: &mut Peer) -> u64 {
    let result = peer
        .rpc(
            Operation::SessionCatalogQuery.as_str(),
            json!({"kind":"get","sessionId":SESSION}),
        )
        .await;
    result["result"]["session"]["revision"].as_u64().unwrap()
}

async fn update(peer: &mut Peer, revision: u64, patch: Value) -> Value {
    peer.rpc(
        Operation::SessionConfigurationUpdate.as_str(),
        json!({"sessionId":SESSION,"expectedRevision":revision,"patch":patch}),
    )
    .await
}

async fn boundary(peer: &mut Peer) -> Value {
    let result = peer
        .rpc(
            Operation::SessionExecutionBoundaryQuery.as_str(),
            json!({"sessionId":SESSION}),
        )
        .await;
    assert_eq!(result["ok"], true, "{result}");
    result["result"].clone()
}

async fn resources(peer: &mut Peer) -> Vec<Value> {
    let result = peer
        .rpc(
            Operation::RuntimeResourceQuery.as_str(),
            json!({"kind":"list_start","sessionId":SESSION}),
        )
        .await;
    assert_eq!(result["ok"], true, "{result}");
    result["result"]["resources"].as_array().unwrap().clone()
}
