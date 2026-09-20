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

use super::{
    javascript_plugins::{package, ready},
    support::{client_probe::ClientFixture, peer::Peer},
};
use maka_plugins::{
    composition::Scope,
    execution::CreateChild,
    fiber::Fiber,
    storage::{Data, Mutation},
};
use maka_runtime::{event::Fact, execution::PermissionMode, tool_call::ToolOrigin};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn sdk_clients_freeze_executor_authority_and_settle_before_delivery() {
    tokio::time::timeout(Duration::from_secs(30), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-plugin-clients-");
    let (provider, mut requests) = super::support::message_recovery::Provider::controlled().await;
    let model = super::support::message_recovery::configure(&fixture, &provider.base_url).await;
    let source = package(
        &fixture.workspace,
        "example.clients",
        "shared",
        include_str!("../fixtures/clients-plugin.mjs"),
        false,
    );
    let host = Host::open(fixture.owner()).await.unwrap();
    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("clients.sock");
    #[cfg(windows)]
    let endpoint =
        std::path::PathBuf::from(format!(r"\\.\pipe\maka-clients-{}", uuid::Uuid::new_v4()));
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop.clone()),
    );
    let mut peer = Peer::new(host.clone(), "plugin-clients").await;
    ready(&mut peer).await;
    assert_eq!(
        peer.rpc("plugin.package.install", json!({"sourcePath":source}))
            .await["ok"],
        true
    );
    ready(&mut peer).await;
    assert_eq!(
        peer.rpc(
            "client.capability.replace",
            publication("desktop", "inspect")
        )
        .await["ok"],
        true
    );
    assert_eq!(
        peer.rpc(
            "session.create",
            json!({
                "sessionId":"parent", "workspace":{"kind":"host_path","path":fixture.workspace},
                "executorId":"example.clients", "permissionMode":"bypass"
            })
        )
        .await["ok"],
        true
    );
    let inspector = Fiber::new("example.clients", "inspector", Scope::Profile).unwrap();
    inspector.begin_loading().unwrap();
    let storage = host.plugin_storage(inspector.context()).unwrap();
    let commands = host
        .authorize_plugin_execution(inspector.context(), &["parent".into()])
        .await
        .unwrap();
    inspector.ready().unwrap();
    inspector.publish().unwrap();
    let child = |id: &str, permission_mode, bound_tools| CreateChild {
        operation_id: id.into(),
        parent_session_id: "parent".into(),
        name: id.into(),
        permission_mode,
        bound_tools,
        instructions: None,
        workspace: None,
        target: None,
    };
    let restricted = commands
        .create_child(child("restricted", Some(PermissionMode::Explore), None))
        .await
        .unwrap();
    let bounded = commands
        .create_child(CreateChild {
            target: Some(maka_plugins::execution::Target::Model {
                model,
                thinking_level: None,
            }),
            ..child(
                "bounded",
                None,
                Some(["ClientsBounded".into()].into_iter().collect()),
            )
        })
        .await
        .unwrap();
    let mut late = Some(Peer::new(host.clone(), "late-provider").await);
    for (session, command) in [
        ("parent", "complete"),
        (restricted.session_id.as_str(), "widen"),
        (bounded.session_id.as_str(), "bounded"),
    ] {
        let started = peer
            .rpc(
                "turn.start",
                json!({"sessionId":session,"turnId":command,"content":{"text":command}}),
            )
            .await;
        assert_eq!(started["ok"], true, "{started}");
        if command == "bounded" {
            for (name, input) in [
                ("tool_search", json!({"query":"ClientsBounded"})),
                ("ClientsBounded", json!({})),
            ] {
                let request = requests.recv().await.unwrap();
                request.reply.send(json!({"index":0,"delta":{"tool_calls":[{
                    "index":0,"id":name,"type":"function","function":{"name":name,"arguments":input.to_string()}
                }]},"finish_reason":"tool_calls"})).unwrap();
            }
            let request = requests.recv().await.unwrap();
            assert!(request.body.to_string().contains("bounded"));
            request
                .reply
                .send(json!({"index":0,"delta":{"content":"bounded"},"finish_reason":"stop"}))
                .unwrap();
        }
        if command == "widen" {
            while storage.read("waiting".into()).await.unwrap().is_none() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let record = peer
                .rpc(
                    "session.catalog.query",
                    json!({"kind":"get","sessionId":session}),
                )
                .await;
            let changed = peer.rpc("session.configuration.update", json!({
                "sessionId":session,"expectedRevision":record["result"]["session"]["revision"],
                "patch":{"permissionMode":"bypass"}
            })).await;
            assert_eq!(changed["result"]["kind"], "committed", "{changed}");
            storage
                .batch(vec![Mutation {
                    key: "continue".into(),
                    expected_revision: None,
                    data: Data::Present(json!(true)),
                }])
                .await
                .unwrap();
        }
        if command != "bounded" {
            let call = capability(&mut peer, "client.capability.call").await;
            assert_eq!(call["arguments"], json!({"command":command}));
            assert!(
                call.get("cwd").is_none(),
                "hostPathAccess none must hide paths"
            );
            if command == "complete" {
                assert_eq!(
                    late.as_mut()
                        .unwrap()
                        .rpc(
                            "client.capability.replace",
                            publication("late", "unexpected")
                        )
                        .await["ok"],
                    true
                );
            }
            peer.send_frame(json!({"kind":"client.capability.accepted","invocationId":call["invocationId"],"admissionEvidence":{"kind":"none"}}));
            if command == "complete" {
                let admitted = capability(&mut peer, "client.capability.admitted").await;
                assert_eq!(admitted["invocationId"], call["invocationId"]);
                peer.send_frame(json!({"kind":"client.capability.result","invocationId":call["invocationId"],"result":{"content":[],"structuredContent":{"inspected":true}}}));
            } else {
                capability(&mut peer, "client.capability.cancel").await;
            }
        }
        loop {
            let state = peer
                .rpc("turn.query", json!({"sessionId":session,"turnId":command}))
                .await;
            if !matches!(
                state["result"]["status"].as_str(),
                Some("created" | "admitted" | "running")
            ) {
                assert_eq!(state["result"]["status"], "completed", "{state}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if command == "complete" {
            late.take().unwrap().close().await;
        }
    }
    inspector
        .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    peer.close().await;
    stop.cancel();
    server.await.unwrap().unwrap();
    cleanup.disarm();
    drop(host);
    let log = fixture.log().await;
    let prefix = log.prefix(500, 4 * 1024 * 1024).await.unwrap();
    let mut pending = std::collections::BTreeSet::new();
    let mut dispatched = 0;
    for row in &prefix.events {
        match &row.event.fact {
            Fact::ToolDispatched {
                operation_id,
                name,
                call,
                ..
            } if name == "mcp__desktop__inspect" => {
                assert!(matches!(call.origin, ToolOrigin::HostSdk { .. }));
                assert_eq!(row.event.invocation.session_id, "parent");
                pending.insert(operation_id.clone());
                dispatched += 1;
            }
            Fact::ToolSettled { operation_id, .. } => {
                pending.remove(operation_id);
            }
            Fact::InvocationEnded { .. } => {
                assert!(pending.is_empty(), "client call outlived invocation")
            }
            _ => {}
        }
    }
    assert_eq!(
        dispatched, 1,
        "denied SDK calls must not commit effect admission"
    );
    log.close().await.unwrap();
}
pub(super) fn publication(server: &str, tool: &str) -> Value {
    json!({"registrationId":server,"offers":[{
        "offerId":server,"version":"1","affinity":"session","hostPathAccess":"none","label":server,
        "tools":[{"serverId":server,"name":tool,"inputSchema":{"type":"object"}}]
    }]})
}
async fn capability(peer: &mut Peer, kind: &str) -> Value {
    loop {
        let frame = peer.frame().await;
        if frame["kind"] == kind {
            return frame;
        }
        assert!(frame.get("requestId").is_none(), "{frame}");
        assert_ne!(
            frame["kind"], "client.capability.admitted",
            "unexpected effect admission"
        );
    }
}
