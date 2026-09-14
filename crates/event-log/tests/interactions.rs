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
use maka_runtime::interaction::{
    ClosureReason, Decision, GrantCapability, GrantScope, GrantTarget, InteractionOutcome,
    InteractionRecord, InteractionRequest,
};
use sqlx::Connection;

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
fn decision(value: Decision, time: u64) -> InteractionOutcome {
    InteractionOutcome::ClientCapabilityDecision {
        decision: value,
        committed_at: time,
    }
}

#[tokio::test]
async fn first_outcome_and_atomic_grant_survive_conflicts_faults_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &serde_json::json!({}), 1)
        .await
        .unwrap();
    let mut wake = log.subscribe_commits();
    let candidate = request("request");
    assert!(log.establish_interaction(&candidate).await.unwrap().matches);
    wake.changed().await.unwrap();
    assert_eq!(
        *wake.borrow_and_update(),
        0,
        "interaction is not an execution event"
    );
    let mut conflict = candidate.clone();
    conflict.run_id = "other_run".into();
    let result = log.establish_interaction(&conflict).await.unwrap();
    assert!(!result.matches);
    assert_eq!(result.record, candidate);
    assert_eq!(
        log.pending_interactions("session").await.unwrap(),
        std::slice::from_ref(&candidate)
    );
    let observation = log
        .observe_session::<serde_json::Value>("session")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        observation.pending_interactions,
        std::slice::from_ref(&candidate)
    );
    assert_eq!(observation.through_sequence, 0);

    // Failure between outcome insertion and grant insertion must roll back both.
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await
    .unwrap();
    sqlx::raw_sql(
        "CREATE TRIGGER fail_grant BEFORE INSERT ON client_capability_session_grants
        BEGIN SELECT RAISE(ABORT, 'injected grant failure'); END;",
    )
    .execute(&mut observer)
    .await
    .unwrap();
    assert!(
        log.commit_interaction_outcome("request", decision(Decision::Allow, 20))
            .await
            .is_err()
    );
    assert_eq!(
        log.interaction("request").await.unwrap(),
        Some(candidate.clone())
    );
    assert!(
        log.client_capability_grant("session", target(&candidate))
            .await
            .unwrap()
            .is_none()
    );
    assert!(!wake.has_changed().unwrap());
    sqlx::raw_sql("DROP TRIGGER fail_grant")
        .execute(&mut observer)
        .await
        .unwrap();

    let (allow, deny) = tokio::join!(
        log.commit_interaction_outcome("request", decision(Decision::Allow, 21)),
        log.commit_interaction_outcome("request", decision(Decision::Deny, 22)),
    );
    let (allow, deny) = (allow.unwrap(), deny.unwrap());
    assert_ne!(allow.matches, deny.matches);
    assert_eq!(allow.record, deny.record);
    let canonical = allow.record;
    let allowed = matches!(
        canonical.outcome,
        Some(InteractionOutcome::ClientCapabilityDecision {
            decision: Decision::Allow,
            ..
        })
    );
    let grant = log
        .client_capability_grant("session", target(&candidate))
        .await
        .unwrap();
    assert_eq!(grant.is_some(), allowed);
    if let Some(grant) = &grant {
        assert_eq!(
            grant.granted_at,
            canonical.outcome.as_ref().unwrap().committed_at()
        );
        assert_eq!(grant.target, *target(&candidate));
    }
    let repeat = log
        .commit_interaction_outcome(
            "request",
            decision(
                if allowed {
                    Decision::Allow
                } else {
                    Decision::Deny
                },
                99,
            ),
        )
        .await
        .unwrap();
    assert!(
        repeat.matches,
        "timestamps do not change semantic answer identity"
    );
    assert_eq!(repeat.record, canonical);
    assert!(log.establish_interaction(&candidate).await.unwrap().matches);
    assert!(
        log.pending_interactions("session")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(log.prefix(32, 65536).await.unwrap().events.is_empty());
    let projection = log
        .session_projection::<serde_json::Value>("session")
        .await
        .unwrap()
        .unwrap();
    assert!(projection.pending_interactions.is_empty());
    assert_eq!(projection.through_sequence, observation.through_sequence);
    observer.close().await.unwrap();
    log.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(log.interaction("request").await.unwrap(), Some(canonical));
    assert_eq!(
        log.client_capability_grant("session", target(&candidate))
            .await
            .unwrap(),
        grant
    );
    log.close().await.unwrap();
}

#[tokio::test]
async fn closure_cannot_grant_and_browser_authority_is_session_provider_contract_origin() {
    let temp = tempfile::tempdir().unwrap();
    let log = EventLog::open(&temp.path().join("runtime.sqlite"))
        .await
        .unwrap();
    let closed = request("closed");
    log.establish_interaction(&closed).await.unwrap();
    let closure = InteractionOutcome::Closure {
        reason: ClosureReason::ProviderDisconnected,
        committed_at: 30,
    };
    log.commit_interaction_outcome("closed", closure.clone())
        .await
        .unwrap();
    let late = log
        .commit_interaction_outcome("closed", decision(Decision::Allow, 31))
        .await
        .unwrap();
    assert!(!late.matches);
    assert_eq!(late.record.outcome, Some(closure));
    assert!(
        log.client_capability_grant("session", target(&closed))
            .await
            .unwrap()
            .is_none()
    );

    let allowed = request("allowed");
    log.establish_interaction(&allowed).await.unwrap();
    log.commit_interaction_outcome("allowed", decision(Decision::Allow, 40))
        .await
        .unwrap();
    let mut other_tool = target(&allowed).clone();
    other_tool.tool_name = "browser_snapshot".into();
    assert!(
        log.client_capability_grant("session", &other_tool)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        log.client_capability_grant("other_session", &other_tool)
            .await
            .unwrap()
            .is_none()
    );
    for variant in 0..3 {
        let mut changed = other_tool.clone();
        match variant {
            0 => changed.provider_id = "other_provider".into(),
            1 => changed.contract_id = "other_contract".into(),
            _ => {
                changed.scope = GrantScope::BrowserOrigin {
                    origin: "https://other.example".into(),
                }
            }
        }
        assert!(
            log.client_capability_grant("session", &changed)
                .await
                .unwrap()
                .is_none()
        );
    }
    let later = request("later");
    log.establish_interaction(&later).await.unwrap();
    log.commit_interaction_outcome("later", decision(Decision::Allow, 50))
        .await
        .unwrap();
    assert_eq!(
        log.client_capability_grant("session", target(&later))
            .await
            .unwrap()
            .unwrap()
            .granted_at,
        40
    );
    let mut invalid = request("invalid");
    invalid.outcome = Some(decision(Decision::Allow, 60));
    assert!(log.establish_interaction(&invalid).await.is_err());
    assert!(log.interaction("invalid").await.unwrap().is_none());
    log.close().await.unwrap();
}
