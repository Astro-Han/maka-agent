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

use maka_event_log::{EventLog, message_resolution::MessageExecution, workhub::stop::StopRequest};
use maka_runtime::{
    artifact::content_digest,
    continuation::{ContinuationClaim, REPLAY_VERSION, ReplayEvidence, RunBoundary, SessionBase},
    event::{EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent},
    execution::{
        CollaborationMode, InvocationConfiguration, OrchestrationMode, PermissionMode, ToolMode,
        WorkspaceIdentity,
    },
    input::InvocationInput,
    workhub::{COORDINATION_SESSION_ID, Delegation, StopOutcome, stop_abort_source},
};
use serde_json::json;

#[path = "continuation/fixtures.rs"]
mod fixtures;
use fixtures::*;

#[tokio::test]
async fn stop_claim_cancellation_and_recovery_keep_the_original_message_and_exact_cause() {
    for scenario in ["pending", "manual", "workhub", "interrupted"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.sqlite");
        let log = EventLog::open(&path).await.unwrap();
        for session in [COORDINATION_SESSION_ID, "session"] {
            log.create_session(session, "create", &json!({}), 1)
                .await
                .unwrap();
        }
        let mut coordinator = opening("coordinator", None);
        coordinator.invocation.session_id = COORDINATION_SESSION_ID.into();
        append(&log, &coordinator).await;
        let mut target = opening("target", None);
        let delegation = Delegation {
            kind: Default::default(),
            action_id: "delegation".into(),
            request_fingerprint: content_digest(b"delegation"),
            source_message_event_id: coordinator.id.clone(),
            target: target.invocation.clone(),
            target_revision: 1,
            delegation_text: "delegated work".into(),
        };
        let action = RuntimeEvent::new(
            coordinator.invocation.clone(),
            Fact::WorkhubDelegated {
                delegation: Box::new(delegation.clone()),
            },
        );
        append(&log, &action).await;
        let pending = log.pending_messages("session").await.unwrap().remove(0);
        if scenario != "pending" {
            let Fact::InvocationOpened { input, .. } = &mut target.fact else {
                unreachable!()
            };
            *input = InvocationInput::Message {
                content: pending.source.message.content.clone(),
                source_messages: vec![pending.source.clone()],
                request_fingerprint: None,
                skill_invocation: None,
            };
            append(&log, &target).await;
        }
        let request = StopRequest {
            action_id: "stop".into(),
            request_fingerprint: content_digest(b"stop"),
            source: coordinator.invocation.clone(),
            target_session_id: "session".into(),
        };
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TRIGGER reject_stop BEFORE INSERT ON workhub_stops BEGIN SELECT RAISE(ABORT,'stop publication failed'); END;").unwrap();
        assert!(log.request_workhub_stop(request.clone()).await.is_err());
        assert!(log.workhub_stop("stop").await.unwrap().is_none());
        if scenario == "pending" {
            assert_eq!(
                log.pending_messages("session").await.unwrap(),
                vec![pending.clone()]
            );
            assert!(matches!(
                log.message_execution("session", &delegation.target_message_id())
                    .await
                    .unwrap(),
                MessageExecution::Pending
            ));
        }
        db.execute_batch("DROP TRIGGER reject_stop;").unwrap();
        let record = log.request_workhub_stop(request.clone()).await.unwrap();
        assert_eq!(
            log.request_workhub_stop(request.clone()).await.unwrap(),
            record
        );
        let mut changed = request.clone();
        changed.request_fingerprint = content_digest(b"another stop");
        assert!(log.request_workhub_stop(changed).await.is_err());
        let mut collision = delegation.clone();
        collision.action_id = "stop".into();
        assert!(
            log.append(
                &EventWrite::plain(RuntimeEvent::new(
                    coordinator.invocation.clone(),
                    Fact::WorkhubDelegated {
                        delegation: Box::new(collision),
                    }
                ))
                .unwrap()
            )
            .await
            .is_err()
        );
        let mut collision = request.clone();
        collision.action_id = "delegation".into();
        assert!(log.request_workhub_stop(collision).await.is_err());

        if scenario == "pending" {
            assert_eq!(
                record.resolution.as_ref().unwrap().outcome,
                StopOutcome::CancelledPending
            );
            assert!(log.pending_messages("session").await.unwrap().is_empty());
            assert!(matches!(
                log.message_execution("session", &delegation.target_message_id())
                    .await
                    .unwrap(),
                MessageExecution::Cancelled
            ));
            assert!(
                log.append(
                    &EventWrite::plain({
                        let mut forbidden = target.clone();
                        let Fact::InvocationOpened { input, .. } = &mut forbidden.fact else {
                            unreachable!()
                        };
                        *input = InvocationInput::Message {
                            content: pending.source.message.content.clone(),
                            source_messages: vec![pending.source.clone()],
                            request_fingerprint: None,
                            skill_invocation: None,
                        };
                        forbidden
                    })
                    .unwrap()
                )
                .await
                .is_err(),
                "cancelled pending work cannot escape through canonical delivery"
            );
        } else {
            assert!(record.resolution.is_none());
            assert_eq!(record.intent.owner.as_ref(), Some(&target.invocation));
            let mut competing = request.clone();
            competing.action_id = "competing-stop".into();
            assert!(log.request_workhub_stop(competing).await.is_err());
            if scenario == "interrupted" {
                append(
                    &log,
                    &RuntimeEvent::new(
                        target.invocation.clone(),
                        Fact::ToolDispatched {
                            operation_id: "unknown".into(),
                            call: maka_runtime::tool_call::ToolCallIdentity::standalone(
                                "call".into(),
                            ),
                            name: "write".into(),
                            input: json!({}),
                        },
                    ),
                )
                .await;
            }
            append(
                &log,
                &RuntimeEvent::new(
                    target.invocation.clone(),
                    Fact::InvocationEnded {
                        outcome: match scenario {
                            "workhub" => InvocationOutcome::Cancelled {
                                source: stop_abort_source("stop"),
                            },
                            "manual" => InvocationOutcome::Cancelled {
                                source: "runtime_cancellation".into(),
                            },
                            _ => InvocationOutcome::Failed {
                                class: "outcome_unknown".into(),
                                message: None,
                            },
                        },
                    },
                ),
            )
            .await;
            if scenario == "workhub" {
                let context = log
                    .context_before_run(&target.invocation, 100, 65536)
                    .await
                    .unwrap();
                let claim = claim(
                    &log,
                    "later-resume",
                    &target,
                    SessionBase {
                        high_water: context.source_evidence.high_water,
                        digest: context.source_evidence.digest,
                    },
                )
                .await;
                let unrelated = opening("unrelated", None);
                append(&log, &unrelated).await;
                close(&log, &unrelated).await;
                let successor = opening("later-resume", Some(claim));
                append(&log, &successor).await;
                close(&log, &successor).await;
                assert!(
                    matches!(log.message_execution("session", &delegation.target_message_id()).await.unwrap(), MessageExecution::Owned(owner) if owner.invocation == successor.invocation)
                );
            }
        }
        close(&log, &coordinator).await;
        let before = serde_json::to_value(log.prefix(100, 1024 * 1024).await.unwrap()).unwrap();
        log.close().await.unwrap();
        drop(db);
        let log = EventLog::open(&path).await.unwrap();
        assert_eq!(
            log.recover_workhub_stops().await.unwrap(),
            usize::from(scenario != "pending")
        );
        let resolved = log.request_workhub_stop(request).await.unwrap();
        assert_eq!(resolved.intent.owner, record.intent.owner);
        assert_eq!(
            resolved.resolution.as_ref().unwrap().outcome,
            match scenario {
                "pending" => StopOutcome::CancelledPending,
                "workhub" => StopOutcome::StopDelivered,
                _ => StopOutcome::AlreadyTerminal,
            }
        );
        assert_eq!(log.resolve_workhub_stop("stop").await.unwrap(), resolved);
        assert_eq!(log.recover_workhub_stops().await.unwrap(), 0);
        assert_eq!(
            serde_json::to_value(log.prefix(100, 1024 * 1024).await.unwrap()).unwrap(),
            before,
            "control recovery cannot modify sealed Runs, replay effects, or touch later roots"
        );
        log.close().await.unwrap();
    }
}
