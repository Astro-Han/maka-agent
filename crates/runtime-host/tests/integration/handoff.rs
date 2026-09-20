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
async fn cooperative_retirement_recovers_frozen_step_without_repeating_effects() {
    let fixture = ClientFixture::new("maka-cooperative-");
    let (provider, mut requests) = super::support::message_recovery::Provider::controlled().await;
    let model = super::support::message_recovery::configure(&fixture, &provider.base_url).await;
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
        model,
        PermissionMode::Bypass,
        ToolMode::Direct,
    );
    let log = fixture.log().await;
    log.create_session("session", "fixture", &configuration, 1)
        .await
        .unwrap();
    log.close().await.unwrap();

    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("cooperative.sock");
    #[cfg(windows)]
    let endpoint = std::path::PathBuf::from(format!(
        r"\\.\pipe\maka-cooperative-{}",
        uuid::Uuid::new_v4()
    ));
    let host = Host::open(fixture.owner()).await.unwrap();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), CancellationToken::new()),
    );
    let (mut peer, hello) = Peer::handshake(host.clone(), "cooperative").await;
    assert_eq!(hello["cooperativeHandoff"], true, "{hello}");
    let publication = json!({"registrationId":"before-upgrade", "offers":[{
        "offerId":"desktop", "version":"1", "affinity":"session", "hostPathAccess":"none",
        "label":"Desktop", "tools":[{"serverId":"desktop", "name":"inspect",
            "inputSchema":{"type":"object"}}]
    }]});
    let registered = peer
        .rpc("client.capability.replace", publication.clone())
        .await;
    assert_eq!(registered["ok"], true, "{registered}");
    let started = peer
        .rpc(
            "turn.start",
            json!({"sessionId":"session", "turnId":"turn",
        "content":{"text":"write once, then finish"}, "maxSteps":4}),
        )
        .await;
    assert_eq!(started["result"]["kind"], "started", "{started}");
    let public_run = started["result"]["turn"]["runId"].clone();
    let first = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    // No safe boundary exists while this request is still outstanding. The
    // bounded preparation must release its ticket without ending the Run.
    let rolled_back = peer
        .rpc(
            "host.upgrade.prepare",
            json!({
                "expectedHostEpoch":hello["hostEpoch"], "allowInterruptActiveTasks":false,
                "allowCooperativeHandoff":true,
            }),
        )
        .await;
    assert_eq!(
        rolled_back["result"]["kind"], "active_tasks",
        "{rolled_back}"
    );
    let probe = Peer::new(host.clone(), "rollback-probe").await;
    probe.close().await;
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        1,
        "rollback must not retry the model"
    );
    first
        .reply
        .send(json!({"index":0,"delta":{"tool_calls":[{
        "index":0,"id":"load-plugin","type":"function","function":{
            "name":"tool_search","arguments":json!({"query":"ScheduledTask"}).to_string()
        }
    }]},"finish_reason":"tool_calls"}))
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        first.body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "ScheduledTask")
    );
    let retire = |mut peer: Peer| {
        let epoch = hello["hostEpoch"].clone();
        tokio::spawn(async move {
            let receipt = peer
                .rpc(
                    "host.upgrade.prepare",
                    json!({
                        "expectedHostEpoch":epoch, "allowInterruptActiveTasks":false,
                        "allowCooperativeHandoff":true,
                    }),
                )
                .await;
            (receipt, peer)
        })
    };
    let mut retiring = retire(peer);
    // Hello admission observes the fence without adding an ordinary command
    // that would itself make cooperative preparation busy.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (probe, hello) = Peer::handshake(host.clone(), "fence-probe").await;
            probe.close().await;
            if hello["kind"] == "draining" {
                break;
            }
            if retiring.is_finished() {
                // A hello that won the Ready race was another live client.
                // Retry only after closing it, preserving interruption consent.
                let (receipt, peer) = (&mut retiring).await.unwrap();
                assert_eq!(receipt["result"]["kind"], "active_tasks", "{receipt}");
                retiring = retire(peer);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    first
        .reply
        .send(json!({"index":0,"delta":{"tool_calls":[{
        "index":0,"id":"write-once","type":"function","function":{
            "name":"Write","arguments":json!({"path":"effect.txt","content":"once"}).to_string()
        }
    }]},"finish_reason":"tool_calls"}))
        .unwrap();
    let (receipt, peer) = retiring.await.unwrap();
    peer.close().await;
    assert_eq!(receipt["result"]["kind"], "prepared", "{receipt}");
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(host);
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("effect.txt")).unwrap(),
        "once"
    );
    assert!(
        requests.try_recv().is_err(),
        "retiring Host must not start another model step"
    );
    let log = fixture.log().await;
    let pending = log.pending_handoffs(0).await.unwrap();
    assert_eq!(pending.len(), 1);
    let source = pending[0].invocation.clone();
    assert_eq!(public_run, source.run_id);
    let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
    assert!(matches!(
        prefix.events.last().unwrap().event.fact,
        Fact::InvocationEnded {
            outcome: InvocationOutcome::HandoffPaused { .. }
        }
    ));
    log.close().await.unwrap();

    let host = Host::open(fixture.owner()).await.unwrap();
    let cancel = CancellationToken::new();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), cancel.clone()),
    );
    // Ready does not depend on a disconnected client. An identical offer from
    // another client cannot substitute for the original admitted owner.
    let mut wrong = Peer::new(host.clone(), "another-desktop").await;
    let registered = wrong
        .rpc("client.capability.replace", publication.clone())
        .await;
    assert_eq!(registered["ok"], true, "{registered}");
    assert!(
        tokio::time::timeout(Duration::from_millis(500), requests.recv())
            .await
            .is_err(),
        "successor ran without its original capability owner"
    );
    wrong.close().await;
    let mut peer = Peer::new(host.clone(), "cooperative").await;
    let mut publication = publication;
    publication["registrationId"] = json!("after-upgrade");
    let registered = peer.rpc("client.capability.replace", publication).await;
    assert_eq!(registered["ok"], true, "{registered}");
    let second = tokio::time::timeout(Duration::from_secs(10), requests.recv())
        .await
        .unwrap()
        .unwrap();
    let tools = second.body["tools"].as_array().unwrap();
    assert!(
        tools
            .iter()
            .any(|tool| tool["function"]["name"] == "tool_search")
    );
    assert!(
        tools
            .iter()
            .all(|tool| tool["function"]["name"] != "ScheduledTask"),
        "successor rediscovers plugins instead of retaining old loaded implementations"
    );
    assert_eq!(
        second.body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool" && message["tool_call_id"] == "write-once")
            .count(),
        1,
        "successor must inherit the settled result"
    );
    second
        .reply
        .send(json!({"index":0,"delta":{"content":"finished"},"finish_reason":"stop"}))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let turn = peer
                .rpc("turn.query", json!({"sessionId":"session","turnId":"turn"}))
                .await;
            assert_eq!(turn["result"]["runId"], public_run, "{turn}");
            assert_ne!(turn["result"]["status"], "failed", "{turn}");
            if turn["result"]["status"] == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    peer.close().await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(host);
    let log = fixture.log().await;
    assert!(log.pending_handoffs(0).await.unwrap().is_empty());
    let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
    assert_eq!(
        prefix
            .events
            .iter()
            .filter(|event| matches!(&event.event.fact,
        Fact::ToolDispatched { name, .. } if name == "Write"))
            .count(),
        1
    );
    let owner = log.handoff_owner(&source).await.unwrap();
    assert_ne!(owner.invocation.run_id, source.run_id);
    assert_eq!(owner.invocation.turn_id, source.turn_id);
    log.close().await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_sealed_turn_uses_public_identity_without_provider_and_survives_restart() {
    for queued in [false, true] {
        let fixture = ClientFixture::new("maka-handoff-stop-");
        let unavailable = SessionModel {
            connection_id: "removed-provider".into(),
            connection_slug: "removed".into(),
            model: "unavailable".into(),
        };
        let provider = if queued {
            Some(super::support::message_recovery::Provider::start().await)
        } else {
            None
        };
        let model = if let Some(provider) = &provider {
            super::support::message_recovery::configure(&fixture, &provider.base_url).await
        } else {
            unavailable.clone()
        };
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
            model,
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
                compaction: maka_runtime::handoff::CompactionBudget::Available,
                replay_base: None,
            }),
        };
        let mut admitted = configuration.invocation_configuration().await.unwrap();
        admitted.model = Some(unavailable);
        for (index, fact) in [
            Fact::InvocationOpened {
                configuration: Some(Box::new(admitted)),
                input: InvocationInput::Message {
                    content: "stop this task".into(),
                    request_fingerprint: None,
                    source_messages: Vec::new(),
                },
            },
            Fact::InvocationEnded {
                outcome: InvocationOutcome::HandoffPaused { pause },
            },
        ]
        .into_iter()
        .enumerate()
        {
            log.append(&EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap())
                .await
                .unwrap();
            if queued && index == 0 {
                let content: maka_runtime::input::MessageInput = "followup work".into();
                log.admit_message(
                    maka_event_log::message_admissions::PendingMessageAdmission {
                        invocation: invocation.clone(),
                        steering_invocation: None,
                        required_tools: Default::default(),
                        admitted_at: 1,
                        source: maka_runtime::message::RootSourceMessage {
                            message: maka_runtime::input::DeliveredMessage {
                                message_id: "followup".into(),
                                submitted_content_digest: content.content_digest().unwrap(),
                                content,
                            },
                            submitted_placement: maka_runtime::message::Placement::NextTurn,
                            disposition: maka_runtime::message::MessageDisposition::Followup,
                            submitted_intent: None,
                        },
                    },
                )
                .await
                .unwrap();
            }
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
            let endpoint = std::path::PathBuf::from(format!(
                r"\\.\pipe\maka-handoff-{}",
                uuid::Uuid::new_v4()
            ));
            let cancel = CancellationToken::new();
            let server = tokio::spawn(
                LocalListener::bind(&endpoint)
                    .unwrap()
                    .serve(host.clone(), cancel.clone()),
            );
            let mut peer = Peer::new(host.clone(), "handoff-stop").await;
            if let Some(provider) = &provider {
                assert_eq!(
                    provider.requests.lock().unwrap().len(),
                    usize::from(reopened),
                    "startup must leave queued work behind the unclaimed seal"
                );
            }
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
            if queued {
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        let message = peer
                            .rpc(
                                "turn.message.execution.query",
                                json!({"sessionId":"session", "messageIds":["followup"]}),
                            )
                            .await;
                        assert_eq!(message["ok"], true, "{message}");
                        let resolution = &message["result"]["resolutions"][0];
                        if resolution["state"] == "owned" {
                            assert_ne!(resolution["turnId"], "turn");
                            let turn = peer
                                .rpc(
                                    "turn.query",
                                    json!({"sessionId":"session", "turnId":resolution["turnId"]}),
                                )
                                .await;
                            assert_ne!(turn["result"]["status"], "failed", "{turn}");
                            if turn["result"]["status"] == "completed" {
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("cancelling a seal must wake its queued successor");
                assert_eq!(provider.as_ref().unwrap().requests.lock().unwrap().len(), 1);
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
            if !queued {
                assert_eq!(
                    prefix.events.len(),
                    4,
                    "only one claim and one cancellation are added"
                );
            }
            assert_eq!(
                prefix
                    .events
                    .iter()
                    .filter(|event| matches!(
                        event.event.fact,
                        Fact::InvocationEnded {
                            outcome: InvocationOutcome::Cancelled { .. }
                        }
                    ))
                    .count(),
                1
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
}
