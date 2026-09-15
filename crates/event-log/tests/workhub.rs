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

use maka_event_log::EventLog;
use maka_runtime::{
    artifact::content_digest,
    event::{EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent},
    input::InvocationInput,
    workhub::{COORDINATION_SESSION_ID, Delegation},
};
use serde_json::json;
fn invocation(session: &str, name: &str) -> Invocation {
    Invocation {
        session_id: session.into(),
        turn_id: format!("turn-{name}"),
        run_id: format!("run-{name}"),
        invocation_id: format!("invocation-{name}"),
    }
}
fn write(invocation: &Invocation, fact: Fact) -> EventWrite {
    EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap()
}
fn message(text: &str) -> Fact {
    Fact::InvocationOpened {
        configuration: None,
        input: InvocationInput::Message {
            content: text.into(),
            request_fingerprint: None,
            source_messages: Vec::new(),
            skill_invocation: None,
        },
    }
}

#[tokio::test]
async fn steering_keeps_delivery_ownership_across_waiting_shells_and_terminal_handoff() {
    use maka_event_log::message_resolution::MessageExecution;
    use maka_runtime::{
        interaction::{
            ClosureReason, InteractionOutcome, InteractionQuestion, InteractionRecord,
            InteractionRequest,
        },
        shell_run::{ShellOutput, ShellRun, ShellState, ShellVisibility},
        workhub::DelegationDelivery,
    };
    for scenario in ["live", "late", "unknown", "orphan", "changed"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.sqlite");
        let mut log = EventLog::open(&path).await.unwrap();
        for session in [COORDINATION_SESSION_ID, "target"] {
            log.create_session(session, "create", &json!({}), 1)
                .await
                .unwrap();
        }
        let coordinator = invocation(COORDINATION_SESSION_ID, "coord");
        let opening = write(&coordinator, message("user authority"));
        log.append(&opening).await.unwrap();
        let target = invocation("target", "existing");
        log.append(&write(&target, message("unrelated original task")))
            .await
            .unwrap();
        let basis = log
            .get_session::<serde_json::Value>("target")
            .await
            .unwrap()
            .unwrap();
        let delegation = Delegation {
            kind: Default::default(),
            description: None,
            delivery: DelegationDelivery::Steering {
                configuration_digest: basis.configuration_digest.clone(),
            },
            action_id: "steer".parse().unwrap(),
            request_fingerprint: content_digest(b"steer"),
            source_message_event_id: opening.event().id.clone(),
            target: target.clone(),
            target_revision: basis.revision,
            delegation_text: "additional task".into(),
        };
        let action = write(
            &coordinator,
            Fact::WorkhubDelegated {
                delegation: Box::new(delegation.clone()),
            },
        );
        let question = InteractionRecord {
            session_id: target.session_id.clone(),
            turn_id: target.turn_id.clone(),
            run_id: target.run_id.clone(),
            request_id: "question".into(),
            created_at: 10,
            outcome: None,
            request: InteractionRequest::Question {
                tool_use_id: "question-tool".into(),
                questions: vec![InteractionQuestion {
                    question: "Continue?".into(),
                    options: ["Yes", "No"]
                        .into_iter()
                        .map(|label| maka_runtime::interaction::QuestionOption {
                            label: label.into(),
                            description: None,
                        })
                        .collect(),
                }],
            },
        };
        log.establish_interaction(&question).await.unwrap();
        assert!(
            log.append(&action).await.is_err(),
            "waiting target cannot accept delegation"
        );
        let candidates = log
            .workhub_candidates(|_, _: &serde_json::Value| true)
            .await
            .unwrap();
        assert!(
            candidates
                .iter()
                .find(|candidate| candidate.session.id == "target")
                .expect("waiting work remains discoverable")
                .session
                .pending_interaction_since
                .is_some()
        );
        assert!(log.pending_messages("target").await.unwrap().is_empty());
        log.commit_interaction_outcome(
            "question",
            InteractionOutcome::Closure {
                reason: ClosureReason::ProducerCancelled,
                committed_at: 11,
            },
        )
        .await
        .unwrap();
        let mut wrong_owner = delegation.clone();
        wrong_owner.target.run_id = "different-run".into();
        assert!(
            log.append(&write(
                &coordinator,
                Fact::WorkhubDelegated {
                    delegation: Box::new(wrong_owner)
                }
            ))
            .await
            .is_err()
        );
        if matches!(scenario, "live" | "orphan") {
            log.create_shell_run(ShellRun {
                id: "shell".into(),
                session_id: "target".into(),
                source_run_id: Some(target.run_id.clone()),
                source_turn_id: target.turn_id.clone(),
                source_tool_call_id: "shell-call".into(),
                visibility: ShellVisibility::Model,
                cwd: temp.path().to_string_lossy().into_owned(),
                command: "background task".into(),
                started_at: 10,
                updated_at: 10,
                timeout_ms: None,
                revision: 1,
                state: ShellState::Starting,
                output: ShellOutput::Pipes {
                    stdout: String::new(),
                    stderr: String::new(),
                    latest_stream: None,
                    stdout_truncated: false,
                    stderr_truncated: false,
                },
            })
            .await
            .unwrap();
        }
        if scenario == "orphan" {
            log.recover_shell_runs(20).await.unwrap();
        }
        if scenario == "changed" {
            let current = log
                .get_session::<serde_json::Value>("target")
                .await
                .unwrap()
                .unwrap();
            let mutation = log
                .update_session_metadata(
                    "target",
                    current.revision,
                    |config: &mut serde_json::Value| {
                        config["name"] = json!("changed target");
                        Ok(())
                    },
                )
                .await
                .unwrap();
            assert!(matches!(
                mutation,
                maka_event_log::sessions::SessionMutation::Committed(_)
            ));
        }
        if scenario == "unknown" {
            log.append(&write(
                &target,
                Fact::ToolDispatched {
                    operation_id: "uncertain".into(),
                    call: maka_runtime::tool_call::ToolCallIdentity::standalone("unknown".into()),
                    name: "write".into(),
                    input: json!({}),
                },
            ))
            .await
            .unwrap();
        }
        if matches!(scenario, "late" | "unknown") {
            log.append(&write(
                &target,
                Fact::InvocationEnded {
                    outcome: if scenario == "late" {
                        InvocationOutcome::Completed
                    } else {
                        InvocationOutcome::Failed {
                            class: "unknown".into(),
                            message: None,
                        }
                    },
                },
            ))
            .await
            .unwrap();
            assert!(
                log.get_session::<serde_json::Value>("target")
                    .await
                    .unwrap()
                    .unwrap()
                    .revision
                    > basis.revision
            );
        }
        if matches!(scenario, "unknown" | "orphan" | "changed") {
            assert!(
                log.append(&action).await.is_err(),
                "sealed/orphaned effects cannot hide behind steering"
            );
            assert!(log.pending_messages("target").await.unwrap().is_empty());
            log.close().await.unwrap();
            continue;
        }
        let sequence = log.append(&action).await.unwrap();
        let pending = log.pending_messages("target").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].invocation, target);
        if scenario == "live" {
            assert_eq!(log.commit_pending_steering(&target).await.unwrap(), 1);
            assert!(
                matches!(log.message_execution("target", &delegation.target_message_id()).await.unwrap(), MessageExecution::Shared(owner) if owner.invocation == target)
            );
        } else {
            // Process exits after admission to an already sealed owner; the normal
            // successor consumes the original source after reopening.
            log.close().await.unwrap();
            log = EventLog::open(&path).await.unwrap();
            assert_eq!(log.pending_messages("target").await.unwrap(), pending);
            let successor = invocation("target", "successor");
            log.append(&write(
                &successor,
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        content: pending[0].source.message.content.clone(),
                        source_messages: vec![pending[0].source.clone()],
                        request_fingerprint: None,
                        skill_invocation: None,
                    },
                },
            ))
            .await
            .unwrap();
            assert!(
                matches!(log.message_execution("target", &delegation.target_message_id()).await.unwrap(), MessageExecution::Owned(owner) if owner.invocation == successor)
            );
        }
        let before = log.prefix(64, 1024 * 1024).await.unwrap().digest;
        log.close().await.unwrap();
        let log = EventLog::open(&path).await.unwrap();
        assert_eq!(log.append(&action).await.unwrap(), sequence);
        assert!(log.pending_messages("target").await.unwrap().is_empty());
        assert_eq!(log.prefix(64, 1024 * 1024).await.unwrap().digest, before);
        log.close().await.unwrap();
    }
}

#[tokio::test]
async fn delegation_is_atomic_and_replay_does_not_reassign_or_requeue() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    for session in [COORDINATION_SESSION_ID, "target", "other", "shell-target"] {
        log.create_session(session, "create", &json!({}), 1)
            .await
            .unwrap();
    }
    let coordinator = invocation(COORDINATION_SESSION_ID, "coordination");
    let source = write(&coordinator, message("original user authority"));
    log.append(&source).await.unwrap();
    let target = invocation("target", "delegated");
    let delegation = Delegation {
        kind: Default::default(),
        description: None,
        delivery: Default::default(),
        action_id: "action".parse().unwrap(),
        request_fingerprint: content_digest(b"bound proposal"),
        source_message_event_id: source.event().id.clone(),
        target: target.clone(),
        target_revision: 1,
        delegation_text: "specific task from the coordinator".into(),
    };
    let action = write(
        &coordinator,
        Fact::WorkhubDelegated {
            delegation: Box::new(delegation.clone()),
        },
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_action BEFORE INSERT ON event_log
        WHEN NEW.kind = 'workhub_delegated' BEGIN SELECT RAISE(ABORT, 'injected action failure'); END;").unwrap();
    assert!(log.append(&action).await.is_err());
    assert!(
        log.workhub_action(&"action".parse().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        log.pending_messages("target").await.unwrap().is_empty(),
        "no target can escape a failed action commit"
    );
    db.execute_batch("DROP TRIGGER fail_action;").unwrap();
    let sequence = log.append(&action).await.unwrap();
    let pending = log.pending_messages("target").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].invocation, target);
    assert_eq!(
        pending[0].source.message.content.text,
        "User request:\noriginal user authority\n\nDelegated task:\nspecific task from the coordinator"
    );
    log.close().await.unwrap();
    drop(db);
    // The action committed, but the target did not start before process exit.
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(log.append(&action).await.unwrap(), sequence);
    assert_eq!(
        log.workhub_action(&"action".parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .event,
        *action.event()
    );
    assert_eq!(log.pending_messages("target").await.unwrap(), pending);
    for (name, id) in [
        ("reserved-alias", &target.invocation_id),
        ("live-alias", &coordinator.invocation_id),
    ] {
        let mut alias = delegation.clone();
        alias.action_id = name.parse().unwrap();
        alias.target = invocation("other", name);
        alias.target.invocation_id = id.clone();
        let result = log
            .append(&write(
                &coordinator,
                Fact::WorkhubDelegated {
                    delegation: Box::new(alias),
                },
            ))
            .await;
        assert!(
            matches!(&result, Err(maka_runtime::event::CommitError::Rejected(reason))
            if reason.contains("execution identity already exists")),
            "{result:?}"
        );
    }
    use maka_runtime::shell_run::{ShellOutput, ShellRun, ShellState, ShellVisibility};
    log.create_shell_run(ShellRun {
        id: "background".into(),
        session_id: "shell-target".into(),
        source_run_id: None,
        source_turn_id: "shell-turn".into(),
        source_tool_call_id: "shell-call".into(),
        visibility: ShellVisibility::Model,
        cwd: temp.path().to_string_lossy().into_owned(),
        command: "background command".into(),
        started_at: 10,
        updated_at: 10,
        timeout_ms: None,
        revision: 1,
        state: ShellState::Starting,
        output: ShellOutput::Pipes {
            stdout: String::new(),
            stderr: String::new(),
            latest_stream: None,
            stdout_truncated: false,
            stderr_truncated: false,
        },
    })
    .await
    .unwrap();
    for orphaned in [false, true] {
        if orphaned {
            assert_eq!(log.recover_shell_runs(20).await.unwrap(), 1);
        }
        let mut shell = delegation.clone();
        shell.action_id = "shell-action".parse().unwrap();
        shell.target = invocation("shell-target", "shell-action");
        assert!(
            log.append(&write(
                &coordinator,
                Fact::WorkhubDelegated {
                    delegation: Box::new(shell)
                }
            ))
            .await
            .is_err()
        );
        assert!(
            log.workhub_action(&"shell-action".parse().unwrap())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            log.pending_messages("shell-target")
                .await
                .unwrap()
                .is_empty()
        );
    }
    let mut changed = delegation.clone();
    changed.target = invocation("other", "replacement");
    assert!(
        log.append(&write(
            &coordinator,
            Fact::WorkhubDelegated {
                delegation: Box::new(changed)
            }
        ))
        .await
        .is_err()
    );
    assert!(
        log.pending_messages("other").await.unwrap().is_empty(),
        "duplicate action identity rolls back its proposed target too"
    );
    let other = invocation("other", "unknown");
    let other_source = write(&other, message("not WorkHub user authority"));
    log.append(&other_source).await.unwrap();
    let mut borrowed = delegation.clone();
    borrowed.action_id = "borrowed".parse().unwrap();
    borrowed.source_message_event_id = other_source.event().id.clone();
    borrowed.target = invocation("other", "borrowed");
    assert!(
        log.append(&write(
            &coordinator,
            Fact::WorkhubDelegated {
                delegation: Box::new(borrowed)
            }
        ))
        .await
        .is_err()
    );
    for fact in [
        Fact::ToolDispatched {
            operation_id: "unknown".into(),
            call: maka_runtime::tool_call::ToolCallIdentity::standalone("call".into()),
            name: "write".into(),
            input: json!({}),
        },
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Failed {
                class: "outcome_unknown".into(),
                message: None,
            },
        },
    ] {
        log.append(&write(&other, fact)).await.unwrap();
    }
    let mut unsafe_target = delegation;
    unsafe_target.action_id = "unsafe-target".parse().unwrap();
    unsafe_target.target = invocation("other", "unsafe-target");
    unsafe_target.target_revision = log
        .get_session::<serde_json::Value>("other")
        .await
        .unwrap()
        .unwrap()
        .revision;
    assert!(
        log.append(&write(
            &coordinator,
            Fact::WorkhubDelegated {
                delegation: Box::new(unsafe_target)
            }
        ))
        .await
        .is_err()
    );
    assert!(log.pending_messages("other").await.unwrap().is_empty());
    // Existing successor delivery consumes the exact admission, not a parallel queue.
    log.append(&write(
        &target,
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: pending[0].source.message.content.clone(),
                request_fingerprint: None,
                source_messages: vec![pending[0].source.clone()],
                skill_invocation: None,
            },
        },
    ))
    .await
    .unwrap();
    for invocation in [&target, &coordinator] {
        log.append(&write(
            invocation,
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Completed,
            },
        ))
        .await
        .unwrap();
    }
    assert!(log.pending_messages("target").await.unwrap().is_empty());
    let before = log.prefix(100, 1024 * 1024).await.unwrap();
    assert_eq!(
        log.append(&action).await.unwrap(),
        sequence,
        "exact receipt replay is legal after source seal"
    );
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(log.append(&action).await.unwrap(), sequence);
    assert!(
        log.pending_messages("target").await.unwrap().is_empty(),
        "receipt replay must not recreate already-delivered work"
    );
    assert_eq!(
        log.prefix(100, 1024 * 1024).await.unwrap().digest,
        before.digest
    );
    assert_eq!(
        log.workhub_action(&"action".parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .sequence,
        sequence
    );
    log.close().await.unwrap();
}
