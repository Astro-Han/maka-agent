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

use maka_event_log::{
    EventLog, message_resolution::MessageExecution, workhub::correction::CorrectionResolution,
};
use maka_runtime::{
    artifact::content_digest,
    event::{EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent},
    input::InvocationInput,
    workhub::{
        ActionId, COORDINATION_SESSION_ID, CorrectionAbort, CorrectionRequest, CorrectionTarget,
        Delegation, DelegationDelivery, DelegationDescription, StopRequest,
    },
};
use serde_json::{Value, json};

fn invocation(session: &str, name: &str) -> Invocation {
    Invocation {
        session_id: session.into(),
        turn_id: format!("turn-{name}"),
        run_id: format!("run-{name}"),
        invocation_id: format!("invocation-{name}"),
    }
}
fn write(owner: &Invocation, fact: Fact) -> EventWrite {
    EventWrite::plain(RuntimeEvent::new(owner.clone(), fact)).unwrap()
}
fn opening(text: &str) -> Fact {
    Fact::InvocationOpened {
        configuration: None,
        input: InvocationInput::Message {
            content: text.into(),
            request_fingerprint: None,
            source_messages: vec![],
        },
    }
}
fn ended() -> Fact {
    Fact::InvocationEnded {
        outcome: InvocationOutcome::Completed,
    }
}
async fn prefix(log: &EventLog, owner: &Invocation) -> Value {
    serde_json::to_value(
        log.scoped_prefix(
            maka_runtime::event::LogScope::Lineage {
                session_id: owner.session_id.clone(),
                run_id: owner.run_id.clone(),
            },
            64,
            1024 * 1024,
        )
        .await
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn correction_recovers_sealed_source_and_atomically_retires_only_its_exact_message() {
    for scenario in ["pending", "owned", "shared", "abort"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.sqlite");
        let log = EventLog::open(&path).await.unwrap();
        let workspace = json!({"hostCwd": "/work"});
        for session in [COORDINATION_SESSION_ID, "old", "new", "third"] {
            log.create_session(
                session,
                "create",
                &json!({"name": session, "workspace": workspace}),
                1,
            )
            .await
            .unwrap();
        }
        let coordinator = invocation(COORDINATION_SESSION_ID, "coordinator");
        let source = write(&coordinator, opening("original authority"));
        log.append(&source).await.unwrap();
        let old_owner = invocation("old", "old");
        if scenario == "shared" {
            log.append(&write(&old_owner, opening("unrelated shared work")))
                .await
                .unwrap();
        }
        let basis = log.get_session::<Value>("old").await.unwrap().unwrap();
        let old = Delegation {
            action_id: ActionId::new("old-assignment").unwrap(),
            kind: Default::default(),
            description: Some(DelegationDescription::Existing { name: "old".into() }),
            delivery: if scenario == "shared" {
                DelegationDelivery::Steering {
                    configuration_digest: basis.configuration_digest,
                }
            } else {
                DelegationDelivery::NewTurn
            },
            request_fingerprint: content_digest(b"old"),
            source_message_event_id: source.event().id.clone(),
            target: old_owner.clone(),
            target_revision: basis.revision,
            delegation_text: "old task".into(),
        };
        let assigned = write(
            &coordinator,
            Fact::WorkhubDelegated {
                delegation: Box::new(old.clone()),
            },
        );
        log.append(&assigned).await.unwrap();
        let pending = log.pending_messages("old").await.unwrap().remove(0);
        if scenario == "owned" {
            log.append(&write(
                &old_owner,
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        content: pending.source.message.content.clone(),
                        request_fingerprint: None,
                        source_messages: vec![pending.source.clone()],
                    },
                },
            ))
            .await
            .unwrap();
        } else if scenario == "shared" {
            assert_eq!(log.commit_pending_steering(&old_owner).await.unwrap(), 1);
        }
        // A separate current user decision authorizes the correction, not old task text.
        log.append(&write(&coordinator, ended())).await.unwrap();
        let correction_owner = invocation(COORDINATION_SESSION_ID, "correction");
        let correction_source = write(&correction_owner, opening("new correction authority"));
        log.append(&correction_source).await.unwrap();
        let request = CorrectionRequest {
            action_id: ActionId::new("correct").unwrap(),
            request_fingerprint: content_digest(b"correction"),
            source: correction_owner.clone(),
            source_message_event_id: correction_source.event().id.clone(),
            replaces_action_id: old.action_id.clone(),
            target: CorrectionTarget::Existing {
                session_id: "new".into(),
                name: "new".into(),
                workspace_digest: content_digest(
                    serde_json::to_string(&workspace).unwrap().as_bytes(),
                ),
            },
            delegation_text: "corrected task".into(),
        };
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TRIGGER reject_intent BEFORE INSERT ON event_log
            WHEN NEW.kind = 'workhub_correction_requested'
            BEGIN SELECT RAISE(ABORT, 'intent fault'); END;",
        )
        .unwrap();
        assert!(
            log.request_workhub_correction(
                request.clone(),
                Some(1),
                None,
                &serde_json::json!({"configuration_digest": "test-target"})
            )
            .await
            .is_err()
        );
        assert!(
            log.workhub_correction(&request.action_id)
                .await
                .unwrap()
                .is_none()
        );
        if matches!(scenario, "pending" | "abort") {
            assert_eq!(log.pending_messages("old").await.unwrap(), vec![pending]);
        }
        db.execute_batch("DROP TRIGGER reject_intent").unwrap();
        let intent = log
            .request_workhub_correction(
                request.clone(),
                Some(1),
                None,
                &serde_json::json!({"configuration_digest": "test-target"}),
            )
            .await
            .unwrap();
        assert_eq!(
            intent.intent.owner.as_ref(),
            (scenario == "owned").then_some(&old_owner)
        );
        assert_eq!(
            log.request_workhub_correction(
                request.clone(),
                Some(1),
                None,
                &serde_json::json!({"configuration_digest": "test-target"})
            )
            .await
            .unwrap(),
            intent
        );
        let mut competing = request.clone();
        competing.action_id = ActionId::new("competing").unwrap();
        assert!(
            log.request_workhub_correction(
                competing,
                Some(1),
                None,
                &serde_json::json!({"configuration_digest": "test-target"})
            )
            .await
            .is_err()
        );
        assert!(
            log.request_workhub_stop(StopRequest {
                action_id: ActionId::new("stop-old").unwrap(),
                request_fingerprint: content_digest(b"stop"),
                source: correction_owner.clone(),
                target_session_id: "old".into(),
            })
            .await
            .is_err()
        );

        let mut replacement = Delegation {
            action_id: request.action_id.clone(),
            kind: Default::default(),
            description: Some(DelegationDescription::Existing { name: "new".into() }),
            delivery: DelegationDelivery::NewTurn,
            request_fingerprint: request.request_fingerprint.clone(),
            source_message_event_id: correction_source.event().id.clone(),
            target: invocation("new", "new"),
            target_revision: 1,
            delegation_text: request.delegation_text.clone(),
        };
        if scenario == "owned" {
            assert!(
                log.finish_workhub_correction::<Value>(
                    &request.action_id,
                    replacement.clone(),
                    None
                )
                .await
                .is_err()
            );
            log.append(&write(&old_owner, ended())).await.unwrap();
        }
        log.append(&write(&correction_owner, ended()))
            .await
            .unwrap();
        let sealed_prefix = prefix(&log, &correction_owner).await;
        log.close().await.unwrap();
        let log = EventLog::open(&path).await.unwrap();
        assert_eq!(
            log.pending_workhub_corrections().await.unwrap(),
            vec![intent.clone()]
        );
        if scenario == "abort" {
            let result = log
                .abort_workhub_correction(&request.action_id, CorrectionAbort::TargetUnavailable)
                .await
                .unwrap();
            assert!(matches!(
                result.resolution,
                Some(CorrectionResolution::Aborted(
                    CorrectionAbort::TargetUnavailable
                ))
            ));
            assert_eq!(
                log.finish_workhub_correction::<Value>(&request.action_id, replacement, None)
                    .await
                    .unwrap(),
                result
            );
            assert!(log.pending_messages("new").await.unwrap().is_empty());
        } else {
            // Renaming while the old owner drains is allowed, but the new fact
            // must capture the actual name at final admission.
            let updated = log
                .update_session_metadata("new", 1, |value: &mut Value| {
                    value["name"] = json!("renamed new");
                    Ok(())
                })
                .await
                .unwrap();
            let maka_event_log::sessions::SessionMutation::Committed(updated) = updated else {
                panic!("metadata conflict")
            };
            replacement.target_revision = updated.revision;
            replacement.description = Some(DelegationDescription::Existing {
                name: "renamed new".into(),
            });
            db.execute_batch(
                "CREATE TRIGGER reject_supersession BEFORE INSERT ON event_log
                WHEN NEW.kind = 'workhub_superseded'
                BEGIN SELECT RAISE(ABORT, 'terminal fault'); END;",
            )
            .unwrap();
            assert!(
                log.finish_workhub_correction::<Value>(
                    &request.action_id,
                    replacement.clone(),
                    None
                )
                .await
                .is_err()
            );
            assert!(
                log.workhub_assignment(&request.action_id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(log.pending_messages("new").await.unwrap().is_empty());
            db.execute_batch("DROP TRIGGER reject_supersession")
                .unwrap();
            let result = log
                .finish_workhub_correction::<Value>(&request.action_id, replacement.clone(), None)
                .await
                .unwrap();
            assert_eq!(
                log.abort_workhub_correction(
                    &request.action_id,
                    CorrectionAbort::TargetUnavailable
                )
                .await
                .unwrap(),
                result
            );
            let assigned = log
                .workhub_assignment(&request.action_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(assigned.coordinator, correction_owner);
            assert_eq!(*assigned.delegation, replacement);
            let pending = log.pending_messages("new").await.unwrap().remove(0);
            assert!(
                pending
                    .source
                    .message
                    .content
                    .text
                    .contains("new correction authority")
            );
            assert!(
                !pending
                    .source
                    .message
                    .content
                    .text
                    .contains("original authority")
            );
            // Replacement assignments must themselves be stoppable, without
            // borrowing or extending the now sealed coordinator.
            let later = invocation(COORDINATION_SESSION_ID, "later");
            let later_source = write(&later, opening("correct replacement then stop"));
            log.append(&later_source).await.unwrap();
            let stopped_assignment = if scenario == "owned" {
                let next = CorrectionRequest {
                    action_id: ActionId::new("correct-again").unwrap(),
                    request_fingerprint: content_digest(b"correct-again"),
                    source: later.clone(),
                    source_message_event_id: later_source.event().id.clone(),
                    replaces_action_id: request.action_id.clone(),
                    target: CorrectionTarget::Existing {
                        session_id: "third".into(),
                        name: "third".into(),
                        workspace_digest: content_digest(
                            serde_json::to_string(&workspace).unwrap().as_bytes(),
                        ),
                    },
                    delegation_text: "corrected again".into(),
                };
                log.request_workhub_correction(
                    next.clone(),
                    Some(1),
                    None,
                    &serde_json::json!({"configuration_digest": "test-target"}),
                )
                .await
                .unwrap();
                let next_assignment = Delegation {
                    action_id: next.action_id.clone(),
                    request_fingerprint: next.request_fingerprint,
                    source_message_event_id: next.source_message_event_id,
                    description: Some(DelegationDescription::Existing {
                        name: "third".into(),
                    }),
                    target: invocation("third", "third"),
                    target_revision: 1,
                    delegation_text: next.delegation_text,
                    ..replacement.clone()
                };
                log.finish_workhub_correction::<Value>(
                    &next.action_id,
                    next_assignment.clone(),
                    None,
                )
                .await
                .unwrap();
                next_assignment
            } else {
                replacement.clone()
            };
            let stopped = log
                .request_workhub_stop(StopRequest {
                    action_id: ActionId::new("stop-new").unwrap(),
                    request_fingerprint: content_digest(b"stop-new"),
                    source: later,
                    target_session_id: stopped_assignment.target.session_id.clone(),
                })
                .await
                .unwrap();
            assert_eq!(
                stopped.intent.delegation_action_id,
                stopped_assignment.action_id
            );
            assert!(log.pending_messages("new").await.unwrap().is_empty());
            assert!(log.pending_messages("third").await.unwrap().is_empty());
        }
        assert!(log.pending_workhub_corrections().await.unwrap().is_empty());
        assert_eq!(prefix(&log, &correction_owner).await, sealed_prefix);
        if scenario == "shared" {
            assert!(matches!(
                log.message_execution("old", &old.target_message_id())
                    .await
                    .unwrap(),
                MessageExecution::Shared(_)
            ));
            assert!(!matches!(
                log.run_boundary("old", &old_owner.run_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .state,
                maka_event_log::turns::InvocationState::Ended { .. }
            ));
        }
        let high = *log.subscribe_commits().borrow();
        assert!(
            log.prepare_transcript(COORDINATION_SESSION_ID, high, 32)
                .await
                .unwrap()
        );
        let headers = log
            .transcript_headers(
                COORDINATION_SESSION_ID,
                &maka_event_log::transcript::TranscriptRead {
                    through: maka_presentation::watermark(high).unwrap(),
                    position: 0,
                    direction: maka_event_log::transcript::TranscriptDirection::Newer,
                    limit: 64,
                },
            )
            .await
            .unwrap();
        let mut correction_rows = vec![];
        for row in headers {
            let bytes = log
                .transcript_fragment(COORDINATION_SESSION_ID, row.sequence, 0, row.total_bytes)
                .await
                .unwrap();
            let message: Value = serde_json::from_slice(&bytes).unwrap();
            if message["actionId"] == request.action_id.as_str() {
                correction_rows.push(message);
            }
        }
        assert_eq!(
            correction_rows.len(),
            if scenario == "abort" { 2 } else { 3 }
        );
        assert_eq!(
            correction_rows[0]["kind"],
            "delegation_replacement_requested"
        );
        assert_eq!(
            correction_rows[0]["replacesDelegationId"],
            assigned.event().id
        );
        if scenario != "abort" {
            assert_eq!(correction_rows[1]["kind"], "delegation_assigned");
            assert_eq!(
                correction_rows[2]["replacementDelegationId"],
                correction_rows[1]["id"]
            );
        }
        log.close().await.unwrap();
    }
}
