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
use maka_event_log::{
    message_resolution::{MessageExecution, MessageResolution},
    observation::StreamFact,
    transcript::{TranscriptDirection, TranscriptRead},
};
use maka_runtime::{
    artifact::content_digest,
    input::DeliveredMessage,
    message::{MessageDisposition, Placement, RootSourceMessage},
    model::{ModelEvent, ModelPart, ModelStep, TextKind},
    workhub::{
        COORDINATION_SESSION_ID, CorrectionRequest, CorrectionTarget, Delegation,
        DelegationDescription,
    },
};
use serde_json::{Value, json};

async fn seal(
    log: &EventLog,
    source: &RuntimeEvent,
) -> (maka_runtime::handoff::HandoffPause, SessionBase) {
    use maka_runtime::handoff::{HandoffIntent, HandoffPause};
    let Fact::InvocationOpened { input, .. } = &source.fact else {
        unreachable!()
    };
    let base = if let Some(claim) = input.inherited_claim() {
        claim.base.clone()
    } else {
        let context = log
            .context_before_run(&source.invocation, 100, 65536)
            .await
            .unwrap();
        SessionBase {
            high_water: context.source_evidence.high_water,
            digest: context.source_evidence.digest,
        }
    };
    let root = match input {
        InvocationInput::Handoff { pause, .. } => &pause.intent.root_run_id,
        _ => &source.invocation.run_id,
    };
    let next = format!("{}-next", source.invocation.run_id);
    let pause = HandoffPause {
        intent: HandoffIntent {
            handoff_id: next.clone(),
            host_epoch: "host".into(),
            root_run_id: root.clone(),
            successor_run_id: next.clone(),
            successor_invocation_id: format!("invocation-{next}"),
            claim_id: next,
        },
        remaining_steps: std::num::NonZeroU16::new(2).unwrap(),
        execution: super::handoff::execution(),
    };
    append(
        log,
        &RuntimeEvent::new(
            source.invocation.clone(),
            Fact::InvocationEnded {
                outcome: InvocationOutcome::HandoffPaused {
                    pause: pause.clone(),
                },
            },
        ),
    )
    .await;
    (pause, base)
}

async fn successor(log: &EventLog, source: &RuntimeEvent) -> RuntimeEvent {
    let (pause, base) = seal(log, source).await;
    let Fact::InvocationOpened { configuration, .. } = &source.fact else {
        unreachable!()
    };
    let claim = claim(log, &pause.intent.claim_id, source, base).await;
    let next = RuntimeEvent::new(
        pause.intent.successor(&source.invocation),
        Fact::InvocationOpened {
            configuration: configuration.clone(),
            input: InvocationInput::Handoff {
                claim: Box::new(claim),
                pause: Box::new(pause),
            },
        },
    );
    append(log, &next).await;
    next
}

#[tokio::test]
async fn handoff_output_keeps_one_public_identity_and_all_physical_evidence_after_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let mut root = opening("root", None);
    let Fact::InvocationOpened {
        input:
            InvocationInput::Message {
                content,
                source_messages,
                ..
            },
        ..
    } = &mut root.fact
    else {
        unreachable!()
    };
    source_messages.push(RootSourceMessage {
        message: DeliveredMessage {
            message_id: "user-message".into(),
            submitted_content_digest: content.content_digest().unwrap(),
            content: content.clone(),
        },
        submitted_placement: Placement::NextTurn,
        disposition: MessageDisposition::TurnStarted,
        skill_invocation: Default::default(),
        submitted_intent: None,
    });
    append(&log, &root).await;
    let mut current = root.clone();
    for text in ["first answer", "second answer"] {
        current = successor(&log, &current).await;
        let step_id = format!("step-{}", current.invocation.run_id);
        if text == "first answer" {
            let content = maka_runtime::input::MessageInput::from("additional direction");
            append(
                &log,
                &RuntimeEvent::new(
                    current.invocation.clone(),
                    Fact::MessageSteered {
                        message: Box::new(DeliveredMessage {
                            message_id: "steered-message".into(),
                            submitted_content_digest: content.content_digest().unwrap(),
                            content,
                        }),
                        skill_invocation: Default::default(),
                    },
                ),
            )
            .await;
        }
        let context = log
            .read_model_context(
                "session",
                Some(&current.invocation.invocation_id),
                100,
                65536,
            )
            .await
            .unwrap();
        append(
            &log,
            &RuntimeEvent::new(
                current.invocation.clone(),
                Fact::ModelRequested {
                    purpose: Some(maka_runtime::context::ModelPurpose::Main),
                    context: None,
                    checkpoint_event_id: None,
                    step_id: step_id.clone(),
                    model_id: "test".into(),
                    source_scope: context.source_evidence.scope,
                    source_high_water: context.source_evidence.high_water,
                    source_digest: context.source_evidence.digest,
                    effective_source_digest: Some(context.effective_source_digest),
                    input_digest: digest('a'),
                    route_identity: digest('b'),
                },
            ),
        )
        .await;
        for event in [
            ModelEvent::PartStarted {
                id: "part".into(),
                text_kind: TextKind::Text,
                provider_options: None,
            },
            ModelEvent::PartDelta {
                id: "part".into(),
                text: text.into(),
                provider_options: None,
            },
            ModelEvent::PartFinished {
                id: "part".into(),
                provider_options: None,
            },
        ] {
            append(
                &log,
                &RuntimeEvent::new(
                    current.invocation.clone(),
                    Fact::ModelObserved {
                        step_id: step_id.clone(),
                        event,
                    },
                ),
            )
            .await;
        }
        append(
            &log,
            &RuntimeEvent::new(
                current.invocation.clone(),
                Fact::ModelCompleted {
                    step_id,
                    output: ModelStep {
                        parts: vec![ModelPart::Text {
                            text_kind: TextKind::Text,
                            text: text.into(),
                            provider_options: None,
                        }],
                        finish_reason: maka_runtime::model::ModelFinishReason::Stop,
                        usage: Default::default(),
                        provider_options: None,
                        response_id: None,
                        model: None,
                        timestamp: None,
                    },
                },
            ),
        )
        .await;
    }
    let terminal = append(
        &log,
        &RuntimeEvent::new(
            current.invocation.clone(),
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Cancelled {
                    source: "user".into(),
                },
            },
        ),
    )
    .await;
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    let session = log.get_session::<Value>("session").await.unwrap().unwrap();
    assert_eq!(
        session
            .execution
            .unwrap()
            .last_message
            .unwrap()
            .preview
            .as_deref(),
        Some("second answer")
    );
    assert_eq!(
        log.latest_continuation_candidate("session").await.unwrap(),
        Some(current.invocation.run_id.clone())
    );
    assert_eq!(
        log.message_resolutions("session", &["user-message".into()])
            .await
            .unwrap(),
        vec![MessageResolution::Owned {
            message_id: "user-message".into(),
            invocation: root.invocation.clone()
        },]
    );
    let MessageExecution::Owned(owner) = log
        .message_execution("session", "user-message")
        .await
        .unwrap()
    else {
        panic!("exclusive owner lost");
    };
    assert_eq!(
        owner.invocation, current.invocation,
        "control must follow the physical owner"
    );
    let mut after = 0;
    let mut texts = Vec::new();
    loop {
        let page = log
            .session_stream_events("session", after, terminal, 2, 4096)
            .await
            .unwrap();
        for event in page.events {
            assert_eq!(event.root_run_id, root.invocation.run_id);
            if let StreamFact::PartDelta { text, .. } = event.fact {
                assert_ne!(event.invocation.run_id, root.invocation.run_id);
                texts.push(text);
            }
        }
        let Some(next) = page.next_after else {
            break;
        };
        assert!(next > after);
        after = next;
    }
    assert_eq!(texts, ["first answer", "second answer"]);
    let rows = transcript(&log, "session").await;
    for text in ["root", "first answer", "second answer"] {
        assert_eq!(
            rows.iter().filter(|row| row["text"] == text).count(),
            1,
            "{text} must appear once"
        );
    }
    assert_eq!(
        rows.iter()
            .filter(|row| row["type"] == "turn_state")
            .count(),
        1,
        "physical pauses do not end the logical Turn"
    );
    assert_eq!(
        log.navigation_fence("session").await.unwrap(),
        Some(maka_presentation::watermark(terminal).unwrap())
    );
    assert_eq!(
        log.navigation_landmarks(
            "session",
            maka_presentation::watermark(terminal).unwrap(),
            64,
            None,
        )
        .await
        .unwrap()
        .len(),
        1
    );
    log.close().await.unwrap();
}

#[tokio::test]
async fn workhub_handoff_keeps_user_authority_and_recovers_correction_after_coordinator_exit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let workspace = json!({"hostCwd": "/work"});
    for session in [COORDINATION_SESSION_ID, "old", "new"] {
        log.create_session(
            session,
            "create",
            &json!({"name": session, "workspace": workspace}),
            1,
        )
        .await
        .unwrap();
    }
    let mut root = opening("original-request", None);
    root.invocation.session_id = COORDINATION_SESSION_ID.into();
    append(&log, &root).await;
    let coordinator = successor(&log, &root).await;
    let delegation = Delegation {
        kind: Default::default(),
        description: Some(DelegationDescription::Existing { name: "old".into() }),
        delivery: Default::default(),
        action_id: "assignment".parse().unwrap(),
        request_fingerprint: content_digest(b"assignment"),
        source_message_event_id: root.id.clone(),
        target: Invocation {
            session_id: "old".into(),
            turn_id: "target-turn".into(),
            run_id: "target-run".into(),
            invocation_id: "target-invocation".into(),
        },
        target_revision: 1,
        delegation_text: "first task".into(),
    };
    let event = RuntimeEvent::new(
        coordinator.invocation.clone(),
        Fact::WorkhubDelegated {
            delegation: Box::new(delegation.clone()),
        },
    );
    append(&log, &event).await;
    let pending = log.pending_messages("old").await.unwrap().remove(0);
    let mut target = opening("target", None);
    target.invocation = delegation.target.clone();
    let Fact::InvocationOpened { input, .. } = &mut target.fact else {
        unreachable!()
    };
    *input = InvocationInput::Message {
        content: pending.source.message.content.clone(),
        source_messages: vec![pending.source],
        request_fingerprint: None,
        skill_invocation: None,
    };
    append(&log, &target).await;
    let request = CorrectionRequest {
        action_id: "correction".parse().unwrap(),
        request_fingerprint: content_digest(b"correction"),
        source: coordinator.invocation.clone(),
        source_message_event_id: root.id.clone(),
        replaces_action_id: delegation.action_id.clone(),
        target: CorrectionTarget::Existing {
            session_id: "new".into(),
            name: "new".into(),
            workspace_digest: content_digest(serde_json::to_string(&workspace).unwrap().as_bytes()),
        },
        delegation_text: "corrected task".into(),
    };
    let mut wrong = request.clone();
    wrong.source_message_event_id = coordinator.id.clone();
    assert!(
        log.request_workhub_correction(wrong, Some(1), None, None::<&()>)
            .await
            .is_err(),
        "a handoff opening is not a new user decision"
    );
    let intent = log
        .request_workhub_correction(request.clone(), Some(1), None, None::<&()>)
        .await
        .unwrap();
    assert_eq!(intent.intent.owner.as_ref(), Some(&target.invocation));
    let tip = successor(&log, &target).await;
    seal(&log, &tip).await;
    assert!(log.pending_messages("old").await.unwrap().is_empty());
    close(&log, &coordinator).await;
    let rows = transcript(&log, COORDINATION_SESSION_ID).await;
    assert!(
        rows.iter().any(|row| row["id"] == event.id),
        "assignment receipt must be projectable from a handoff Run"
    );
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    let replacement = Delegation {
        action_id: request.action_id.clone(),
        request_fingerprint: request.request_fingerprint.clone(),
        source_message_event_id: root.id,
        delegation_text: request.delegation_text,
        target: Invocation {
            session_id: "new".into(),
            turn_id: "new-turn".into(),
            run_id: "new-run".into(),
            invocation_id: "new-invocation".into(),
        },
        description: Some(DelegationDescription::Existing { name: "new".into() }),
        ..delegation
    };
    assert!(
        matches!(
            log.finish_workhub_correction::<Value>(&request.action_id, replacement.clone(), None)
                .await,
            Err(maka_event_log::StoreError::SessionBusy)
        ),
        "a physical seal is not retirement of delegated work"
    );
    log.cancel_handoff(
        &tip.invocation,
        maka_runtime::event::CancellationCause::WorkhubCorrection {
            action_id: request.action_id.clone(),
        },
    )
    .await
    .unwrap();
    let resolved = log
        .finish_workhub_correction::<Value>(&request.action_id, replacement.clone(), None)
        .await
        .unwrap();
    assert_eq!(
        log.finish_workhub_correction::<Value>(&request.action_id, replacement, None)
            .await
            .unwrap(),
        resolved
    );
    let pending = log.pending_messages("new").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(
        pending[0]
            .source
            .message
            .content
            .text
            .contains("original-request")
    );
    assert!(
        pending[0]
            .source
            .message
            .content
            .text
            .contains("corrected task")
    );
    let after = transcript(&log, COORDINATION_SESSION_ID).await;
    assert!(
        after.len() > rows.len(),
        "recovered correction publishes its immutable receipt"
    );
    let mut stop = opening("stop-request", None);
    stop.invocation.session_id = COORDINATION_SESSION_ID.into();
    append(&log, &stop).await;
    let stop = successor(&log, &stop).await;
    let stopped = log
        .request_workhub_stop(maka_runtime::workhub::StopRequest {
            action_id: "stop-replacement".parse().unwrap(),
            request_fingerprint: content_digest(b"stop-replacement"),
            source: stop.invocation.clone(),
            target_session_id: "new".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        stopped.resolution,
        Some(maka_runtime::workhub::StopResolution::CancelledPending)
    ));
    assert!(log.pending_messages("new").await.unwrap().is_empty());
    close(&log, &stop).await;
    assert!(transcript(&log, COORDINATION_SESSION_ID).await.len() > after.len());
    log.close().await.unwrap();
}

#[tokio::test]
async fn stop_recovers_a_frozen_owner_through_handoffs_but_never_manual_resume() {
    use maka_runtime::workhub::{StopOutcome, StopRequest};
    for admitted_before_handoff in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stop.sqlite");
        let log = EventLog::open(&path).await.unwrap();
        for session in [COORDINATION_SESSION_ID, "session"] {
            log.create_session(session, "fixture", &json!({"name":session}), 1)
                .await
                .unwrap();
        }
        let mut coordinator = opening("coordinator", None);
        coordinator.invocation.session_id = COORDINATION_SESSION_ID.into();
        append(&log, &coordinator).await;
        let mut target = opening("target", None);
        let delegation = Delegation {
            kind: Default::default(),
            description: None,
            delivery: Default::default(),
            action_id: "assignment".parse().unwrap(),
            request_fingerprint: content_digest(b"assignment"),
            source_message_event_id: coordinator.id.clone(),
            target: target.invocation.clone(),
            target_revision: 1,
            delegation_text: "finish this work".into(),
        };
        append(
            &log,
            &RuntimeEvent::new(
                coordinator.invocation.clone(),
                Fact::WorkhubDelegated {
                    delegation: Box::new(delegation),
                },
            ),
        )
        .await;
        let pending = log.pending_messages("session").await.unwrap().remove(0);
        let Fact::InvocationOpened { input, .. } = &mut target.fact else {
            unreachable!()
        };
        *input = InvocationInput::Message {
            content: pending.source.message.content.clone(),
            source_messages: vec![pending.source],
            request_fingerprint: None,
            skill_invocation: None,
        };
        append(&log, &target).await;
        let request = StopRequest {
            action_id: "stop".parse().unwrap(),
            request_fingerprint: content_digest(b"stop"),
            source: coordinator.invocation.clone(),
            target_session_id: "session".into(),
        };
        if admitted_before_handoff {
            log.request_workhub_stop(request.clone()).await.unwrap();
        }
        let first = successor(&log, &target).await;
        let tip = successor(&log, &first).await;
        seal(&log, &tip).await;
        let intent = log.request_workhub_stop(request.clone()).await.unwrap();
        assert!(intent.resolution.is_none());
        assert_eq!(
            intent.intent.owner.as_ref(),
            Some(if admitted_before_handoff {
                &target.invocation
            } else {
                &tip.invocation
            })
        );
        assert!(matches!(
            log.resolve_workhub_stop(&request.action_id).await,
            Err(maka_event_log::StoreError::SessionBusy)
        ));
        assert!(log.has_pending_handoff("session").await.unwrap());
        let sealed = log.prefix(100, 128 * 1024).await.unwrap();
        close(&log, &coordinator).await;
        log.close().await.unwrap();
        let log = EventLog::open(&path).await.unwrap();
        let before_recovery = log.prefix(100, 128 * 1024).await.unwrap();
        let database = rusqlite::Connection::open(&path).unwrap();
        database.execute_batch("CREATE TRIGGER reject_stop_recovery BEFORE INSERT ON event_log
            WHEN NEW.kind='workhub_stop_resolved' BEGIN SELECT RAISE(ABORT,'resolution failed'); END;").unwrap();
        assert!(log.recover_workhub_stops().await.is_err());
        assert_eq!(
            log.prefix(100, 128 * 1024).await.unwrap().digest,
            before_recovery.digest,
            "failed receipt publication must roll back its cancellation and claim"
        );
        database
            .execute_batch("DROP TRIGGER reject_stop_recovery;")
            .unwrap();
        drop(database);
        assert_eq!(log.recover_workhub_stops().await.unwrap(), 1);
        let resolved = log.workhub_stop(&request.action_id).await.unwrap().unwrap();
        assert_eq!(
            resolved.intent, intent.intent,
            "recovery must not rewrite the frozen physical owner"
        );
        assert_eq!(
            resolved.resolution.as_ref().unwrap().outcome(),
            StopOutcome::StopDelivered
        );
        let cancelled = log.handoff_owner(&target.invocation).await.unwrap();
        assert!(
            matches!(cancelled.state.terminal_outcome(), Some(InvocationOutcome::Cancelled { source })
            if source == &maka_runtime::workhub::stop_abort_source(&request.action_id))
        );
        assert!(!log.has_pending_handoff("session").await.unwrap());
        let after = log.prefix(100, 128 * 1024).await.unwrap();
        assert_eq!(
            serde_json::to_value(&after.events[..sealed.events.len()]).unwrap(),
            serde_json::to_value(&sealed.events).unwrap()
        );
        let source = after
            .events
            .iter()
            .find(|event| {
                event.event.invocation == cancelled.invocation
                    && matches!(event.event.fact, Fact::InvocationOpened { .. })
            })
            .unwrap()
            .event
            .clone();
        let base = cancelled.input.inherited_claim().unwrap().base.clone();
        let manual = opening("manual", Some(claim(&log, "manual", &source, base).await));
        append(&log, &manual).await;
        assert_eq!(
            log.handoff_owner(&target.invocation)
                .await
                .unwrap()
                .invocation,
            cancelled.invocation
        );
        assert_eq!(
            log.resolve_workhub_stop(&request.action_id).await.unwrap(),
            resolved
        );
        assert_eq!(log.recover_workhub_stops().await.unwrap(), 0);
        close(&log, &manual).await;
        log.close().await.unwrap();
    }
}

async fn transcript(log: &EventLog, session: &str) -> Vec<Value> {
    let through = *log.subscribe_commits().borrow();
    while !log.prepare_transcript(session, through, 32).await.unwrap() {}
    let headers = log
        .transcript_headers(
            session,
            &TranscriptRead {
                through: maka_presentation::watermark(through).unwrap(),
                position: 0,
                direction: TranscriptDirection::Newer,
                limit: 100,
            },
        )
        .await
        .unwrap();
    let mut rows = Vec::new();
    for row in headers {
        rows.push(
            serde_json::from_slice(
                &log.transcript_fragment(session, row.sequence, 0, row.total_bytes)
                    .await
                    .unwrap(),
            )
            .unwrap(),
        );
    }
    rows
}
