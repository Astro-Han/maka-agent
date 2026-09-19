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

use maka_event_log::{EventLog, workhub::stop::StopRequest};
use maka_runtime::{
    artifact::content_digest,
    continuation::{ContinuationClaim, REPLAY_VERSION, ReplayEvidence, RunBoundary, SessionBase},
    event::{EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent},
    execution::{
        BehaviorId, CollaborationMode, InvocationConfiguration, PermissionMode, ToolMode,
        WorkspaceIdentity,
    },
    input::InvocationInput,
    tool_call::{ToolCallIdentity, ToolRejection},
    workhub::{COORDINATION_SESSION_ID, Delegation, ResumeOrigin, resumed_turn_id},
};
use serde_json::json;

#[path = "continuation/fixtures.rs"]
mod fixtures;
use fixtures::*;

#[tokio::test]
async fn linked_resume_is_atomic_and_follows_its_message_not_other_delegations_or_tool_input() {
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
    let rejected = |id: &str| {
        RuntimeEvent::new(
            coordinator.invocation.clone(),
            Fact::ToolRejected {
                operation_id: id.into(),
                call: ToolCallIdentity::standalone(id.into()),
                name: "unavailable".into(),
                input: json!({"workhub_resume":{"action_id":"resume"}}),
                reason: ToolRejection::Unavailable,
            },
        )
    };
    append(&log, &rejected("before")).await;
    assert!(
        log.workhub_action(&"resume".parse().unwrap())
            .await
            .unwrap()
            .is_none()
    );

    let mut source = None;
    let mut observed = None;
    for id in ["completed-delegation", "failed-delegation"] {
        let mut target = opening(id, None);
        let revision = log
            .get_session::<serde_json::Value>("session")
            .await
            .unwrap()
            .unwrap()
            .revision;
        let delegation = Delegation {
            kind: Default::default(),
            description: None,
            delivery: Default::default(),
            action_id: id.parse().unwrap(),
            request_fingerprint: content_digest(id.as_bytes()),
            source_message_event_id: coordinator.id.clone(),
            target: target.invocation.clone(),
            target_revision: revision,
            delegation_text: "delegated work".into(),
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
        if id == "completed-delegation" {
            close(&log, &target).await;
        } else {
            let observation = RuntimeEvent::new(
                coordinator.invocation.clone(),
                Fact::WorkhubResumeObserved {
                    resume: Box::new(ResumeOrigin {
                        action_id: "observation".parse().unwrap(),
                        request_fingerprint: digest('f'),
                        coordinator: coordinator.invocation.clone(),
                        delegation_action_id: id.parse().unwrap(),
                    }),
                    target: target.invocation.clone(),
                },
            );
            append(&log, &observation).await;
            observed = Some(observation);
            append(
                &log,
                &RuntimeEvent::new(
                    target.invocation.clone(),
                    Fact::InvocationEnded {
                        outcome: InvocationOutcome::Failed {
                            class: "model_error".into(),
                            message: None,
                        },
                    },
                ),
            )
            .await;
            source = Some(target);
        }
    }
    let source = source.unwrap();
    let context = log
        .context_before_run(&source.invocation, 100, 65536)
        .await
        .unwrap();
    let claim = claim(
        &log,
        "resume-claim",
        &source,
        SessionBase {
            high_water: context.source_evidence.high_water,
            digest: context.source_evidence.digest,
        },
    )
    .await;
    let mut resumed = opening(
        &resumed_turn_id(&"resume".parse().unwrap()),
        Some(claim.clone()),
    );
    let Fact::InvocationOpened {
        input: InvocationInput::Continuation { workhub_resume, .. },
        ..
    } = &mut resumed.fact
    else {
        unreachable!()
    };
    *workhub_resume = Some(ResumeOrigin {
        action_id: "resume".parse().unwrap(),
        request_fingerprint: digest('f'),
        coordinator: coordinator.invocation.clone(),
        delegation_action_id: "failed-delegation".parse().unwrap(),
    });
    let mut wrong = resumed.clone();
    if let Fact::InvocationOpened {
        input:
            InvocationInput::Continuation {
                workhub_resume: Some(origin),
                ..
            },
        ..
    } = &mut wrong.fact
    {
        origin.delegation_action_id = "completed-delegation".parse().unwrap();
    }
    assert!(
        log.append(&EventWrite::plain(wrong).unwrap())
            .await
            .is_err()
    );

    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_resume BEFORE INSERT ON event_log
        WHEN json_extract(NEW.event_json,'$.fact.input.kind')='continuation'
        BEGIN SELECT RAISE(ABORT,'injected resume failure'); END;",
    )
    .unwrap();
    assert!(
        log.append(&EventWrite::plain(resumed.clone()).unwrap())
            .await
            .is_err()
    );
    assert!(
        log.workhub_action(&"resume".parse().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        log.continuation_for_source(&claim.source)
            .await
            .unwrap()
            .is_none()
    );
    db.execute_batch("DROP TRIGGER reject_resume;").unwrap();
    append(&log, &resumed).await;
    assert_eq!(
        log.workhub_action(&"resume".parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .event
            .id,
        resumed.id
    );
    append(&log, &rejected("after")).await;
    assert!(
        log.request_workhub_stop(StopRequest {
            action_id: "resume".parse().unwrap(),
            request_fingerprint: digest('f'),
            source: coordinator.invocation.clone(),
            target_session_id: "session".into(),
        })
        .await
        .is_err()
    );
    close(&log, &resumed).await;
    close(&log, &coordinator).await;
    let before = serde_json::to_value(log.prefix(100, 65536).await.unwrap()).unwrap();
    log.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    append(&log, &resumed).await;
    append(&log, observed.as_ref().unwrap()).await;
    assert_eq!(
        log.workhub_action(&"resume".parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .event
            .id,
        resumed.id
    );
    assert_eq!(
        log.workhub_action(&"observation".parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .event
            .id,
        observed.unwrap().id
    );
    assert_eq!(
        serde_json::to_value(log.prefix(100, 65536).await.unwrap()).unwrap(),
        before
    );
    log.close().await.unwrap();
}
