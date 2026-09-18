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
use maka_runtime::{event::Fact, execution::PermissionMode};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
mod fault;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn sdk_files_preserve_permissions_ceilings_and_durable_settlement() {
    tokio::time::timeout(Duration::from_secs(30), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-plugin-files-");
    let (provider, mut requests) = super::support::message_recovery::Provider::controlled().await;
    let model = super::support::message_recovery::configure(&fixture, &provider.base_url).await;
    std::fs::write(
        fixture.workspace.join("source.txt"),
        "zero\none\ntwo\nthree",
    )
    .unwrap();
    std::fs::write(fixture.workspace.join("many.txt"), "match\n".repeat(200)).unwrap();
    std::fs::write(
        fixture.workspace.parent().unwrap().join("outside.txt"),
        "private",
    )
    .unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(1u32.to_be_bytes());
    png.extend(1u32.to_be_bytes());
    std::fs::write(fixture.workspace.join("image.png"), png).unwrap();
    let source = package(
        &fixture.workspace,
        "example.files",
        "shared",
        include_str!("../fixtures/files-plugin.mjs"),
        false,
    );
    let host = Host::open(fixture.owner()).await.unwrap();
    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("files.sock");
    #[cfg(windows)]
    let endpoint =
        std::path::PathBuf::from(format!(r"\\.\pipe\maka-files-{}", uuid::Uuid::new_v4()));
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop.clone()),
    );
    let mut peer = Peer::new(host.clone(), "plugin-files").await;
    ready(&mut peer).await;
    let installed = peer
        .rpc("plugin.package.install", json!({"sourcePath":source}))
        .await;
    assert_eq!(installed["ok"], true, "{installed}");
    ready(&mut peer).await;
    let created = peer
        .rpc(
            "session.create",
            json!({
                "sessionId":"parent", "workspace":{"kind":"host_path","path":fixture.workspace},
                "executorId":"example.files", "permissionMode":"ask"
            }),
        )
        .await;
    assert_eq!(created["ok"], true, "{created}");
    let inspector = Fiber::new("example.files", "inspector", Scope::Profile).unwrap();
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
    let ceiling = commands
        .create_child(CreateChild {
            target: Some(maka_plugins::execution::ChildTarget::Model {
                model,
                thinking_level: None,
            }),
            ..child(
                "ceiling",
                None,
                Some(["Read".into(), "FilesBounded".into()].into_iter().collect()),
            )
        })
        .await
        .unwrap();
    for (index, (session, command)) in [
        ("parent", "complete"),
        ("parent", "complete"),
        (restricted.session_id.as_str(), "restricted"),
        (restricted.session_id.as_str(), "widen"),
        (ceiling.session_id.as_str(), "ceiling"),
    ]
    .into_iter()
    .enumerate()
    {
        let turn = format!("files-{index}");
        let started = peer
            .rpc(
                "turn.start",
                json!({"sessionId":session,"turnId":turn,"content":{"text":command}}),
            )
            .await;
        assert_eq!(started["ok"], true, "{started}");
        if command == "ceiling" {
            for (name, input) in [
                ("tool_search", json!({"query":"FilesBounded"})),
                ("FilesBounded", json!({})),
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
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let session_state = peer
                .rpc(
                    "session.catalog.query",
                    json!({"kind":"get","sessionId":session}),
                )
                .await;
            let changed = peer.rpc("session.configuration.update", json!({
                "sessionId":session, "expectedRevision":session_state["result"]["session"]["revision"],
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
        loop {
            let state = peer
                .rpc("turn.query", json!({"sessionId":session,"turnId":turn}))
                .await;
            assert_eq!(state["ok"], true, "{state}");
            if !matches!(
                state["result"]["status"].as_str(),
                Some("admitted" | "created" | "running")
            ) {
                assert_eq!(state["result"]["status"], "completed", "{state}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    assert!(!fixture.workspace.join("forbidden.txt").exists());
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("result.txt")).unwrap(),
        "after\n"
    );
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
    for session in ["parent", &restricted.session_id, &ceiling.session_id] {
        while !log
            .prepare_transcript(session, prefix.high_water, 32)
            .await
            .unwrap()
        {}
    }
    let mut pending = std::collections::BTreeSet::new();
    let mut writes = 0;
    for row in &prefix.events {
        match &row.event.fact {
            Fact::ToolDispatched {
                operation_id, name, ..
            } => {
                assert!(pending.insert(operation_id.clone()));
                if name == "Write" {
                    writes += 1;
                }
            }
            Fact::ToolSettled { operation_id, .. } => {
                assert!(pending.remove(operation_id));
            }
            Fact::InvocationEnded { .. } => assert!(
                pending.is_empty(),
                "SDK operation outlived invocation settlement"
            ),
            _ => {}
        }
    }
    assert!(
        writes >= 2,
        "filesystem effects bypassed the canonical journal"
    );
    log.close().await.unwrap();
}
