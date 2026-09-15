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

async fn successor(log: &EventLog, source: &RuntimeEvent) -> RuntimeEvent {
    use maka_runtime::handoff::{HandoffIntent, HandoffPause};
    let Fact::InvocationOpened {
        configuration,
        input,
    } = &source.fact
    else {
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
            64
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
    assert_eq!(log.pending_messages("old").await.unwrap().len(), 1);
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
        log.request_workhub_correction(wrong, Some(1), None)
            .await
            .is_err(),
        "a handoff opening is not a new user decision"
    );
    let intent = log
        .request_workhub_correction(request.clone(), Some(1), None)
        .await
        .unwrap();
    assert!(intent.intent.owner.is_none());
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
