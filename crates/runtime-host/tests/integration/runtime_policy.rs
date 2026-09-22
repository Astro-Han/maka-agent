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

use super::support::client_probe::ClientFixture;
use maka_runtime::execution::{SandboxMode, ToolMode};
use maka_runtime_host::session::SessionConfiguration;
use serde_json::Value;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_mode_is_frozen_at_creation_and_survives_host_reopen() {
    use super::support::{
        message_recovery::{Provider, configure},
        peer::Peer,
    };
    use maka_runtime::event::Fact;
    use maka_runtime_host::server::{Host, local::LocalListener};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    let fixture = ClientFixture::new("maka-tool-mode-");
    let provider = Provider::start().await;
    let model = configure(&fixture, &provider.base_url).await;
    let create = |id| {
        json!({"sessionId":id, "workspace":{"kind":"host_path","path":fixture.workspace},
        "modelTarget":{"kind":"explicit","connectionId":model.connection_id,
            "connectionSlug":model.connection_slug,"model":model.model}})
    };
    let sessions = [
        ("old", ToolMode::Direct),
        ("enabled", ToolMode::CodeMode),
        ("disabled", ToolMode::Direct),
    ];
    let mut saved = Vec::new();
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("policy.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-policy-{}", uuid::Uuid::new_v4()));
        let cancel = CancellationToken::new();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), cancel.clone()),
        );
        let mut peer = Peer::new(host.clone(), "policy-client").await;
        for (index, (id, _)) in sessions.iter().enumerate() {
            if !reopened && index > 0 {
                let result = peer.rpc("runtime.policy.mutate", json!({"expectedRevision":index-1,
                    "operation":{"kind":"set_chat_defaults","value":{"sandboxMode":"workspace-write","codeModeEnabled":index == 1}}})).await;
                assert_eq!(result["result"]["kind"], "committed", "{result}");
            }
            let result = peer.rpc("session.create", create(id)).await;
            assert_eq!(result["ok"], true, "{result}");
        }
        // Both turns run after the global switch was turned back off.
        let started = peer
            .rpc(
                "turn.start",
                json!({"sessionId":"enabled", "turnId":if reopened {"second"} else {"first"},
            "content":{"text":"hello"},"maxSteps":1}),
            )
            .await;
        assert_eq!(started["ok"], true, "{started}");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let turn = peer.rpc("turn.query", json!({"sessionId":"enabled", "turnId":if reopened {"second"} else {"first"}})).await;
                if turn["result"]["status"] == "completed" { break; }
                assert_ne!(turn["result"]["status"], "failed", "{turn}");
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        peer.close().await;
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(host);
        let log = fixture.log().await;
        for (index, (id, mode)) in sessions.iter().enumerate() {
            let record = log
                .get_session::<SessionConfiguration>(id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(record.configuration.tool_mode, *mode);
            if reopened {
                assert_eq!(record.configuration, saved[index]);
            } else {
                saved.push(record.configuration);
            }
        }
        let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
        let openings: Vec<_> = prefix
            .events
            .iter()
            .filter_map(|event| match &event.event.fact {
                Fact::InvocationOpened {
                    configuration: Some(configuration),
                    ..
                } => Some(configuration.tool_mode),
                _ => None,
            })
            .collect();
        assert_eq!(
            openings,
            vec![ToolMode::CodeMode; if reopened { 2 } else { 1 }]
        );
        log.close().await.unwrap();
    }
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert!(
            request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "exec")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_settings_cas_preserves_session_defaults_and_exact_reopen() {
    let fixture = ClientFixture::new("maka-runtime-policy-");
    fixture
        .run("--runtime-policy-workspace", false, "runtime-policy-passed")
        .await;
    let saved: Value = serde_json::from_slice(
        &std::fs::read(fixture.workspace.join("runtime-policy-fixture.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["finalSettings"]["policy"]["revision"], 4);
    let log = fixture.log().await;
    assert!(log.prefix(8, 4096).await.unwrap().events.is_empty());
    let mut records = Vec::new();
    for (id, permission) in [
        ("runtime-policy-old", SandboxMode::WorkspaceWrite),
        ("runtime-policy-inherited", SandboxMode::DangerFullAccess),
        ("runtime-policy-explicit", SandboxMode::WorkspaceWrite),
    ] {
        let record = log
            .get_session::<SessionConfiguration>(id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.configuration.sandbox_mode, permission);
        assert_eq!(
            record.configuration.tool_mode,
            if id == "runtime-policy-old" {
                ToolMode::Direct
            } else {
                ToolMode::CodeMode
            }
        );
        assert_eq!(record.configuration.thinking_level, None);
        records.push(record);
    }
    log.close().await.unwrap();
    fixture
        .run(
            "--runtime-policy-workspace",
            true,
            "runtime-policy-reopened",
        )
        .await;
    let reopened = fixture.log().await;
    assert!(reopened.prefix(8, 4096).await.unwrap().events.is_empty());
    for record in records {
        let after = reopened
            .get_session::<SessionConfiguration>(&record.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after, record);
    }
    reopened.close().await.unwrap();
}
