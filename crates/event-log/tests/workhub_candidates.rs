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
use maka_event_log::sessions::ManagedSession;
use maka_plugins::{composition::Scope, storage::Namespace};
use maka_runtime::event::{
    EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent,
};
use serde::Deserialize;

#[derive(Deserialize, serde::Serialize)]
struct Configuration {
    eligible: bool,
}

#[tokio::test]
async fn candidates_rank_all_pages_by_canonical_activity_including_blocked_work() {
    let temp = tempfile::tempdir().unwrap();
    let log = EventLog::open(&temp.path().join("events.sqlite"))
        .await
        .unwrap();
    for index in 0..40 {
        log.create_session(
            &format!("session-{index:02}"),
            "create",
            &Configuration {
                eligible: index != 39,
            },
            index + 1,
        )
        .await
        .unwrap();
    }
    log.set_session_archived::<Configuration>("session-38", true, 100)
        .await
        .unwrap();
    // Enough newer managed Sessions to fill the entire visible window must
    // not displace ordinary targets, even when their configuration is eligible.
    for index in 0..32 {
        let session_id = format!("managed-{index}");
        log.create_session(
            &session_id,
            "create",
            &Configuration { eligible: true },
            300_000,
        )
        .await
        .unwrap();
        log.reserve_managed_session(&ManagedSession {
            session_id,
            manager: Namespace::new("example.workflow", Scope::Profile).unwrap(),
            fingerprint: "create".into(),
        })
        .await
        .unwrap();
    }
    // An old Session becomes most recent through a real message, not metadata edits.
    for index in [0, 36, 37] {
        let invocation = Invocation {
            session_id: format!("session-{index:02}"),
            turn_id: format!("turn-{index}"),
            run_id: format!("run-{index}"),
            invocation_id: format!("invocation-{index}"),
        };
        let write = |fact| {
            let mut event = RuntimeEvent::new(invocation.clone(), fact);
            event.recorded_at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(200 + index);
            EventWrite::plain(event).unwrap()
        };
        log.append(&write(Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: "actual activity".into(),
                request_fingerprint: None,
                source_messages: Vec::new(),
                skill_invocation: None,
            },
        }))
        .await
        .unwrap();
        if index == 36 {
            log.append(&write(Fact::ToolDispatched {
                operation_id: "unsettled".into(),
                call: maka_runtime::tool_call::ToolCallIdentity::standalone("call".into()),
                name: "write".into(),
                input: serde_json::json!({}),
            }))
            .await
            .unwrap();
        }
        if index != 37 {
            log.append(&write(Fact::InvocationEnded {
                outcome: if index == 36 {
                    InvocationOutcome::Failed {
                        class: "outcome_unknown".into(),
                        message: None,
                    }
                } else {
                    InvocationOutcome::Completed
                },
            }))
            .await
            .unwrap();
        }
    }
    let eligible = |record: &maka_event_log::sessions::SessionRecord<Configuration>| {
        record.configuration.eligible
    };
    let candidates = log
        .workhub_candidates(|_, config: &Configuration| config.eligible)
        .await
        .unwrap();
    assert!(
        log.workhub_candidate("managed-0", eligible)
            .await
            .unwrap()
            .is_none()
    );
    let expected = ["session-37", "session-36", "session-00"]
        .map(str::to_string)
        .into_iter()
        .chain((7..=35).rev().map(|index| format!("session-{index:02}")))
        .collect::<Vec<_>>();
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.session.id.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        maka_event_log::workhub::activity_at(&candidates[0].session),
        237_000
    );
    assert!(
        log.workhub_candidate("session-36", eligible)
            .await
            .unwrap()
            .is_none(),
        "unknown effects prevent new delegation, not discovery"
    );
    log.close().await.unwrap();
}
