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
use maka_runtime::event::EventWrite;
use maka_runtime::interaction::{
    ClosureReason, GrantCapability, GrantScope, GrantTarget, InteractionOutcome, InteractionRecord,
    InteractionRequest,
};

fn request(id: &str) -> InteractionRecord {
    InteractionRecord {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        request_id: id.into(),
        created_at: 10,
        request: InteractionRequest::ClientCapability {
            tool_use_id: "tool-use".into(),
            target: GrantTarget {
                provider_id: "provider".into(),
                contract_id: "contract".into(),
                server_id: "desktop_browser".into(),
                tool_name: "browser_navigate".into(),
                capability: GrantCapability::Browser,
                scope: GrantScope::BrowserOrigin {
                    origin: "https://example.com".into(),
                },
            },
        },
        outcome: None,
    }
}
fn target(record: &InteractionRecord) -> &GrantTarget {
    let InteractionRequest::ClientCapability { target, .. } = &record.request else {
        panic!("expected capability request")
    };
    target
}
#[tokio::test]
async fn run_closure_guards_every_terminal_and_restart_closes_even_sealed_runs() {
    use maka_runtime::event::{Fact, Invocation, InvocationOutcome, RuntimeEvent};
    let temp = tempfile::tempdir().unwrap();
    let log = EventLog::open(&temp.path().join("runtime.sqlite"))
        .await
        .unwrap();
    let invocation = Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    log.append(
        &EventWrite::plain(
            (RuntimeEvent::new(
                invocation.clone(),
                Fact::InvocationOpened {
                    configuration: None,
                    input: maka_runtime::input::InvocationInput::Code {
                        source: "test".into(),
                    },
                },
            ))
            .clone(),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    for variant in 0..4 {
        let mut candidate = request(&format!("request_{variant}"));
        match variant {
            1 => candidate.session_id = "other_session".into(),
            2 => candidate.turn_id = "other_turn".into(),
            3 => candidate.run_id = "other_run".into(),
            _ => {}
        }
        log.establish_interaction(&candidate).await.unwrap();
    }
    for outcome in [
        InvocationOutcome::Completed,
        InvocationOutcome::Cancelled {
            source: "test".into(),
        },
        InvocationOutcome::Failed {
            class: "test".into(),
            message: None,
        },
    ] {
        assert!(
            log.append(
                &EventWrite::plain(
                    (RuntimeEvent::new(invocation.clone(), Fact::InvocationEnded { outcome }))
                        .clone()
                )
                .unwrap()
            )
            .await
            .is_err()
        );
    }
    let mut wake = log.subscribe_commits();
    assert_eq!(
        log.close_run_interactions(&invocation, ClosureReason::TurnTerminal, 50)
            .await
            .unwrap(),
        1
    );
    wake.changed().await.unwrap();
    assert_eq!(*wake.borrow_and_update(), 1);
    assert_eq!(
        log.close_run_interactions(&invocation, ClosureReason::HostRestarted, 60)
            .await
            .unwrap(),
        0
    );
    assert!(!wake.has_changed().unwrap());
    assert_eq!(
        log.interaction("request_0").await.unwrap().unwrap().outcome,
        Some(InteractionOutcome::Closure {
            reason: ClosureReason::TurnTerminal,
            committed_at: 50
        })
    );
    for variant in 1..4 {
        assert!(
            log.interaction(&format!("request_{variant}"))
                .await
                .unwrap()
                .unwrap()
                .outcome
                .is_none()
        );
    }
    log.append(
        &EventWrite::plain(
            (RuntimeEvent::new(
                invocation.clone(),
                Fact::InvocationEnded {
                    outcome: InvocationOutcome::Completed,
                },
            ))
            .clone(),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    // Canonical requests remain independent of execution admission: startup must
    // close a legacy pending request even when its execution Run is sealed.
    log.establish_interaction(&request("legacy")).await.unwrap();
    // Exercise multiple bounded ID pages, beyond the Session projection limit.
    for index in 0..130 {
        log.establish_interaction(&request(&format!("bulk_{index:03}")))
            .await
            .unwrap();
    }
    let before = log.prefix(10, 65536).await.unwrap().high_water;
    assert_eq!(log.close_abandoned_interactions(70).await.unwrap(), 134);
    assert_eq!(log.close_abandoned_interactions(80).await.unwrap(), 0);
    assert_eq!(
        log.interaction("legacy").await.unwrap().unwrap().outcome,
        Some(InteractionOutcome::Closure {
            reason: ClosureReason::HostRestarted,
            committed_at: 70
        })
    );
    assert_eq!(
        log.interaction("request_0").await.unwrap().unwrap().outcome,
        Some(InteractionOutcome::Closure {
            reason: ClosureReason::TurnTerminal,
            committed_at: 50
        })
    );
    assert_eq!(log.prefix(10, 65536).await.unwrap().high_water, before);
    assert!(
        log.client_capability_grant("session", target(&request("legacy")))
            .await
            .unwrap()
            .is_none()
    );
    let mut invalid = invocation;
    invalid.turn_id.clear();
    assert!(
        log.close_run_interactions(&invalid, ClosureReason::TurnTerminal, 90)
            .await
            .is_err()
    );
    assert!(log.close_abandoned_interactions(u64::MAX).await.is_err());
    log.close().await.unwrap();
}
