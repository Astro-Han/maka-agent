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
use super::support::{client_probe::ClientFixture, peer::Peer};
use maka_protocol::session::{PermissionMode, WorkspaceProjection, WorkspaceTarget};
use maka_runtime::{
    artifact::content_digest,
    continuation::{REPLAY_VERSION, ReplayEvidence},
    event::{EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent},
    execution::ToolMode,
    handoff::{HandoffExecution, HandoffIntent, HandoffPause, HandoffTools},
};
use maka_runtime_host::{
    server::{Host, local::LocalListener},
    session::{PreparedSession, SessionModel},
};
use serde_json::json;
use std::{num::NonZeroU16, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_sealed_turn_uses_public_identity_without_provider_and_survives_restart() {
    let fixture = ClientFixture::new("maka-handoff-stop-");
    let log = fixture.log().await;
    let cwd = fixture.workspace.to_string_lossy().into_owned();
    let configuration = PreparedSession::new(
        serde_json::from_value(json!({
            "sessionId":"session", "workspace":{"kind":"host_path", "path":cwd},
            "modelTarget":{"kind":"default"}
        }))
        .unwrap(),
    )
    .unwrap()
    .bind(
        WorkspaceProjection {
            target: WorkspaceTarget::HostPath { path: cwd.clone() },
            host_cwd: cwd,
        },
        SessionModel {
            connection_id: "removed-provider".into(),
            connection_slug: "removed".into(),
            model: "unavailable".into(),
        },
        PermissionMode::Explore,
        ToolMode::Direct,
    );
    log.create_session("session", "fixture", &configuration, 1)
        .await
        .unwrap();
    let invocation = Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "public-run".into(),
        invocation_id: "source-invocation".into(),
    };
    let pause = HandoffPause {
        intent: HandoffIntent {
            handoff_id: "handoff".into(),
            host_epoch: "old-host".into(),
            root_run_id: invocation.run_id.clone(),
            successor_run_id: "physical-successor".into(),
            successor_invocation_id: "successor-invocation".into(),
            claim_id: "claim".into(),
        },
        remaining_steps: NonZeroU16::new(1).unwrap(),
        execution: Box::new(HandoffExecution {
            replay: ReplayEvidence {
                version: REPLAY_VERSION,
                digest: content_digest(b"admission"),
                route_identity: content_digest(b"missing-provider"),
            },
            context: None,
            provider_options: json!({}),
            main_output_limit: None,
            supports_vision: false,
            tools: HandoffTools {
                catalog_digest: content_digest(b"unavailable-tools"),
                loaded: Default::default(),
            },
            compaction_attempted: false,
            replay_base: None,
        }),
    };
    for fact in [
        Fact::InvocationOpened {
            configuration: Some(configuration.invocation_configuration().await.unwrap()),
            input: InvocationInput::Message {
                content: "stop this task".into(),
                request_fingerprint: None,
                source_messages: Vec::new(),
                skill_invocation: None,
            },
        },
        Fact::InvocationEnded {
            outcome: InvocationOutcome::HandoffPaused { pause },
        },
    ] {
        log.append(&EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap())
            .await
            .unwrap();
    }
    let sealed = log.prefix(100, 128 * 1024).await.unwrap();
    log.close().await.unwrap();
    let mut result = None;
    let mut settled = None;
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("handoff.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-handoff-{}", uuid::Uuid::new_v4()));
        let cancel = CancellationToken::new();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), cancel.clone()),
        );
        let mut peer = Peer::new(host.clone(), "handoff-stop").await;
        let turn = peer
            .rpc(
                "turn.query",
                json!({"sessionId":"session", "turnId":"turn"}),
            )
            .await;
        assert_eq!(turn["result"]["runId"], "public-run", "{turn}");
        assert_eq!(
            turn["result"]["status"],
            if reopened { "cancelled" } else { "running" },
            "{turn}"
        );
        let wrong = peer
            .rpc(
                "turn.stop",
                json!({
                    "sessionId":"session", "turnId":"turn", "runId":"physical-successor"
                }),
            )
            .await;
        assert_eq!(wrong["error"]["code"], "operation_conflict", "{wrong}");
        let after_wrong = peer
            .rpc(
                "turn.query",
                json!({"sessionId":"session", "turnId":"turn"}),
            )
            .await;
        assert_eq!(
            after_wrong, turn,
            "wrong public identity must not claim or cancel the seal"
        );
        for _ in 0..2 {
            let stopped = peer
                .rpc(
                    "turn.stop",
                    json!({
                        "sessionId":"session", "turnId":"turn", "runId":"public-run"
                    }),
                )
                .await;
            assert_eq!(stopped["ok"], true, "{stopped}");
            assert_eq!(stopped["result"]["status"], "cancelled", "{stopped}");
            assert_eq!(stopped["result"]["runId"], "public-run", "{stopped}");
            if let Some(expected) = &result {
                assert_eq!(&stopped, expected);
            } else {
                result = Some(stopped);
            }
        }
        peer.close().await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(host);
        let log = fixture.log().await;
        let prefix = log.prefix(100, 128 * 1024).await.unwrap();
        assert_eq!(
            prefix.events.len(),
            4,
            "only one claim and one cancellation are added"
        );
        assert_eq!(
            serde_json::to_value(&prefix.events[..2]).unwrap(),
            serde_json::to_value(&sealed.events).unwrap()
        );
        assert!(matches!(
            &prefix.events[2].event.fact,
            Fact::InvocationOpened {
                input: InvocationInput::Handoff { .. },
                ..
            }
        ));
        assert!(
            matches!(&prefix.events[3].event.fact, Fact::InvocationEnded {
            outcome: InvocationOutcome::Cancelled { source }
        } if source == "runtime_cancellation")
        );
        if let Some(expected) = &settled {
            assert_eq!(&prefix.digest, expected);
        } else {
            settled = Some(prefix.digest);
        }
        log.close().await.unwrap();
    }
}
