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
use maka_runtime::{execution::ToolMode, workhub::COORDINATION_SESSION_ID};
use maka_runtime_host::session::SessionConfiguration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn correction_recovery_aborts_unresolvable_creation_without_blocking_host_startup() {
    use maka_runtime::{
        artifact::content_digest,
        event::{EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent},
        input::InvocationInput,
        workhub::{ActionId, CorrectionRequest, CorrectionTarget, CreateSpec, Delegation},
    };
    use maka_runtime_host::server::{Host, local::LocalListener};
    let fixture = ClientFixture::new("maka-correction-recovery-");
    let log = fixture.log().await;
    for session in [COORDINATION_SESSION_ID, "old"] {
        log.create_session(session, "create", &serde_json::json!({"name": session}), 1)
            .await
            .unwrap();
    }
    let source = Invocation {
        session_id: COORDINATION_SESSION_ID.into(),
        turn_id: "decision".into(),
        run_id: "coordinator".into(),
        invocation_id: "coordinator".into(),
    };
    let opening = RuntimeEvent::new(
        source.clone(),
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: "correct this task".into(),
                request_fingerprint: None,
                source_messages: vec![],
                skill_invocation: None,
            },
        },
    );
    log.append(&EventWrite::plain(opening.clone()).unwrap())
        .await
        .unwrap();
    let delegated = Delegation {
        action_id: ActionId::new("old-action").unwrap(),
        kind: Default::default(),
        description: None,
        delivery: Default::default(),
        request_fingerprint: content_digest(b"delegation"),
        source_message_event_id: opening.id.clone(),
        target: Invocation {
            session_id: "old".into(),
            turn_id: "old-turn".into(),
            run_id: "old-run".into(),
            invocation_id: "old-run".into(),
        },
        target_revision: 1,
        delegation_text: "old task".into(),
    };
    log.append(
        &EventWrite::plain(RuntimeEvent::new(
            source.clone(),
            Fact::WorkhubDelegated {
                delegation: Box::new(delegated.clone()),
            },
        ))
        .unwrap(),
    )
    .await
    .unwrap();
    let action_id = ActionId::new("correct").unwrap();
    let request = CorrectionRequest {
        action_id: action_id.clone(),
        request_fingerprint: content_digest(b"correction"),
        source: source.clone(),
        source_message_event_id: opening.id,
        replaces_action_id: delegated.action_id,
        target: CorrectionTarget::Created {
            session_id: maka_runtime::workhub::created_session_id(&action_id),
            name: "replacement".into(),
            spec: CreateSpec {
                title: "replacement".into(),
                workspace: maka_runtime::execution::WorkspaceTarget::HostPath {
                    path: fixture.workspace.to_string_lossy().into_owned(),
                },
                defaults: None,
            },
        },
        delegation_text: "replacement task".into(),
    };
    log.request_workhub_correction(request.clone(), None, None)
        .await
        .unwrap();
    log.append(
        &EventWrite::plain(RuntimeEvent::new(
            source,
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Completed,
            },
        ))
        .unwrap(),
    )
    .await
    .unwrap();
    log.close().await.unwrap();
    // Disable the domain before restart: accepted Host settlement must not wait
    // for a live plugin. No default model is available at either restart.
    let log = fixture.log().await;
    let mut ledger = log.plugin_composition().await.unwrap();
    ledger.extend(&[maka_plugins::composition::Operation::Update {
        entry_id: "maka.workhub".into(),
        patch: maka_plugins::composition::EntryPatch {
            disabled: Some(true),
            ..Default::default()
        },
    }]);
    log.commit_plugin_state(ledger, None).await.unwrap();
    log.close().await.unwrap();
    for _ in 0..2 {
        let host = Host::open(fixture.owner()).await.unwrap();
        let mut peer = super::support::peer::Peer::new(host.clone(), "disabled-workhub").await;
        let candidates = peer
            .rpc("workhub.coordination.candidates", serde_json::json!({}))
            .await;
        assert_eq!(
            candidates["error"]["code"], "operation_unavailable",
            "{candidates}"
        );
        peer.close().await;
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("recovery.sock");
        #[cfg(windows)]
        let endpoint = std::path::PathBuf::from(format!(
            r"\\.\pipe\maka-correction-{}",
            uuid::Uuid::new_v4()
        ));
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host, cancellation)
            .await
            .unwrap();
        let log = fixture.log().await;
        let record = log.workhub_correction(&action_id).await.unwrap().unwrap();
        assert!(matches!(
            record.resolution,
            Some(
                maka_event_log::workhub::correction::CorrectionResolution::Aborted(
                    maka_runtime::workhub::CorrectionAbort::TargetUnavailable
                )
            )
        ));
        assert!(log.workhub_assignment(&action_id).await.unwrap().is_none());
        assert!(
            log.get_session::<serde_json::Value>(request.target.session_id())
                .await
                .unwrap()
                .is_none()
        );
        assert!(log.pending_messages("old").await.unwrap().is_empty());
        log.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_workhub_routing_runs_once_and_replays_after_reopen() {
    for flag in [
        "--workhub-delegation-workspace",
        "--workhub-creation-workspace",
        "--workhub-selection-workspace",
        "--workhub-stop-workspace",
        "--workhub-steering-workspace",
        "--workhub-resume-workspace",
        "--workhub-correction-workspace",
        "--workhub-correction-creation-workspace",
    ] {
        let fixture = ClientFixture::new("maka-workhub-delegation-");
        fixture.run(flag, false, "workhub-delegation-passed").await;
        let log = fixture.log().await;
        let before = log.prefix(256, 512 * 1024).await.unwrap();
        use maka_runtime::event::Fact;
        assert_eq!(
            before
                .events
                .iter()
                .filter(|row| matches!(row.event.fact, Fact::WorkhubDelegated { .. }))
                .count(),
            1
        );
        assert_eq!(
            before
                .events
                .iter()
                .filter(|row| matches!(row.event.fact, Fact::InvocationOpened { .. }))
                .count(),
            if flag == "--workhub-resume-workspace" || flag.starts_with("--workhub-correction") {
                4
            } else {
                2
            }
        );
        let delegation = before
            .events
            .iter()
            .find_map(|row| match &row.event.fact {
                Fact::WorkhubDelegated { delegation } => Some(delegation),
                _ => None,
            })
            .unwrap();
        if flag == "--workhub-steering-workspace" {
            assert!(matches!(
                log.message_execution(
                    &delegation.target.session_id,
                    &delegation.target_message_id()
                )
                .await
                .unwrap(),
                maka_event_log::message_resolution::MessageExecution::Shared(_)
            ));
        }
        assert!(
            log.pending_messages(&delegation.target.session_id)
                .await
                .unwrap()
                .is_empty()
        );
        log.close().await.unwrap();
        fixture.run(flag, true, "workhub-delegation-reopened").await;
        let log = fixture.log().await;
        assert_eq!(
            serde_json::to_value(log.prefix(256, 512 * 1024).await.unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        log.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_workhub_answer_scopes_read_and_desktop_calls_without_replaying_effects() {
    let fixture = ClientFixture::new("maka-workhub-answer-");
    fixture
        .run("--workhub-answer-workspace", false, "workhub-answer-passed")
        .await;
    let log = fixture.log().await;
    let before = log.prefix(256, 512 * 1024).await.unwrap();
    use maka_runtime::event::Fact;
    let prompt = before
        .events
        .iter()
        .find_map(|row| match &row.event.fact {
            Fact::InvocationOpened {
                configuration: Some(configuration),
                ..
            } => configuration.system_prompt.as_ref(),
            _ => None,
        })
        .expect("WorkHub must freeze its plugin-owned policy");
    assert!(
        prompt
            .sources
            .iter()
            .any(|source| source.package_id == "maka.workhub" && !source.activation.is_empty())
    );
    for row in &before.events {
        if row.event.invocation.session_id == COORDINATION_SESSION_ID
            && matches!(row.event.fact, Fact::ModelRequested { .. })
        {
            let surface = log
                .request_composition(COORDINATION_SESSION_ID, &row.event.id)
                .await
                .unwrap()
                .expect("model request composition");
            assert!(
                prompt
                    .sources
                    .iter()
                    .all(|source| surface.sources.contains(source)),
                "each request must retain the admitted policy provenance"
            );
        }
    }
    let composition = before
        .events
        .iter()
        .find_map(|row| match &row.event.fact {
            Fact::InvocationOpened {
                configuration: Some(configuration),
                ..
            } => configuration.tool_composition.as_ref(),
            _ => None,
        })
        .expect("WorkHub must retain its admitted Desktop owner");
    assert!(
        composition.skills_digest.is_none(),
        "WorkHub has no Skills handlers"
    );
    assert!(!composition.clients.offers.is_empty());
    assert!(composition.clients.offers.iter().all(|offer| matches!(
        offer,
        maka_runtime::capability::ClientOffer::Pinned {
            affinity: maka_runtime::capability::PinnedAffinity::Session,
            ..
        }
    )));
    assert_eq!(
        before
            .events
            .iter()
            .filter(|row| matches!(row.event.fact, Fact::InvocationOpened { .. }))
            .count(),
        3
    );
    assert!(
        before
            .events
            .iter()
            .any(|row| matches!(row.event.fact, Fact::ToolDispatched { .. }))
    );
    log.close().await.unwrap();
    fixture
        .run(
            "--workhub-answer-workspace",
            true,
            "workhub-answer-reopened",
        )
        .await;
    let log = fixture.log().await;
    assert_eq!(
        serde_json::to_value(log.prefix(256, 512 * 1024).await.unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    log.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_workhub_identity_model_cas_and_readonly_query_survive_reopen() {
    let fixture = ClientFixture::new("maka-workhub-client-");
    fixture
        .run("--workhub-workspace", false, "workhub-passed")
        .await;
    let log = fixture.log().await;
    let before = log
        .get_session::<SessionConfiguration>(COORDINATION_SESSION_ID)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before.configuration.tool_profile,
        Some(maka_protocol::session::SessionToolProfile::WorkhubCoordinationV2)
    );
    assert_eq!(before.configuration.tool_mode, ToolMode::Direct);
    assert!(!before.archived);
    assert!(
        log.prefix(8, 4096).await.unwrap().events.is_empty(),
        "WorkHub control state is not fabricated execution history"
    );
    log.close().await.unwrap();
    fixture
        .run("--workhub-workspace", true, "workhub-reopened")
        .await;
    let log = fixture.log().await;
    assert_eq!(
        log.get_session::<SessionConfiguration>(COORDINATION_SESSION_ID)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert!(log.prefix(8, 4096).await.unwrap().events.is_empty());
    log.close().await.unwrap();
}
