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

use maka_event_log::{EventLog, StoreError};
use maka_runtime::{
    event::{EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent},
    input::InvocationInput,
    tool_call::ToolCallIdentity,
};

fn opening(session: &str, run: &str, invocation: &str, text: &str) -> RuntimeEvent {
    RuntimeEvent::new(
        Invocation {
            session_id: session.into(),
            turn_id: "logical-turn".into(),
            run_id: run.into(),
            invocation_id: invocation.into(),
        },
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: text.into(),
                request_fingerprint: None,
                skill_invocation: Default::default(),
                source_messages: Vec::new(),
            },
        },
    )
}

async fn append(log: &EventLog, event: RuntimeEvent) {
    log.append(&EventWrite::plain(event).unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn exact_run_cuts_bind_identity_and_bytes_without_admitting_unproven_replay() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let first = opening("session", "source", "source-invocation", "中文 source");
    let bytes = serde_json::to_vec(&first).unwrap().len();
    append(&log, first.clone()).await;
    let before = log
        .run_prefix("session", "source", None, 1, bytes)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.invocation, first.invocation);
    assert_eq!(before.high_water, 1);
    assert_eq!(before.events[0].sequence, 1);
    assert!(matches!(
        log.run_prefix("session", "source", None, 1, bytes - 1)
            .await,
        Err(StoreError::PrefixTooLarge)
    ));
    assert!(matches!(
        log.run_prefix("session", "source", None, 0, bytes).await,
        Err(StoreError::PrefixTooLarge)
    ));

    // The same Run name in another Session cannot enter this source or its budget.
    append(
        &log,
        opening("other", "source", "unrelated", &"x".repeat(16_384)),
    )
    .await;
    append(
        &log,
        RuntimeEvent::new(
            first.invocation.clone(),
            Fact::ToolDispatched {
                operation_id: "unsettled".into(),
                call: ToolCallIdentity::standalone("call".into()),
                name: "write".into(),
                input: serde_json::json!({}),
            },
        ),
    )
    .await;
    let active = log
        .run_prefix("session", "source", None, 2, 16_384)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        active.high_water, 2,
        "Run ordinal, not root ledger position"
    );
    assert_eq!(
        active.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        [1, 3]
    );
    assert_ne!(before.digest, active.digest);
    // Reading immutable evidence does not erase an unresolved effect or declare it safe.
    assert!(matches!(
        active.events[1].event.fact,
        Fact::ToolDispatched { .. }
    ));

    append(
        &log,
        RuntimeEvent::new(
            first.invocation.clone(),
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Failed {
                    class: "outcome_unknown".into(),
                    message: None,
                },
            },
        ),
    )
    .await;
    append(
        &log,
        opening(
            "session",
            "successor",
            "successor-invocation",
            "later branch",
        ),
    )
    .await;
    let latest = log
        .run_prefix("session", "source", None, 3, 16_384)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.high_water, 3);
    assert_eq!(latest.events.last().unwrap().sequence, 4);
    assert_eq!(
        log.turn_boundary("session", "logical-turn")
            .await
            .unwrap()
            .unwrap()
            .invocation
            .run_id,
        "successor"
    );
    assert_eq!(
        log.run_boundary("session", "source")
            .await
            .unwrap()
            .unwrap()
            .invocation,
        first.invocation
    );
    assert!(
        log.run_prefix("absent", "source", None, 0, 0)
            .await
            .unwrap()
            .is_none()
    );
    for invalid in [0, 4, 9_007_199_254_740_992, u64::MAX] {
        assert!(matches!(
            log.run_prefix("session", "source", Some(invalid), 100, 16_384)
                .await,
            Err(StoreError::InvalidTransition(_))
        ));
    }
    log.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    let frozen = log
        .run_prefix("session", "source", Some(2), 2, 16_384)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frozen.digest, active.digest);
    assert_eq!(frozen.high_water, active.high_water);
    assert_eq!(
        serde_json::to_value(frozen.events).unwrap(),
        serde_json::to_value(active.events).unwrap()
    );
    assert_eq!(
        log.run_prefix("session", "source", Some(1), 1, bytes)
            .await
            .unwrap()
            .unwrap()
            .digest,
        before.digest
    );
    assert!(matches!(
        log.run_prefix("session", "source", Some(3), 2, 16_384)
            .await,
        Err(StoreError::PrefixTooLarge)
    ));
    log.close().await.unwrap();

    // Source authentication binds stored bytes, not reserialized semantic JSON.
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute(
            "UPDATE event_log SET event_json = event_json || ' ' WHERE event_id = ?",
            [&first.id],
        )
        .unwrap();
    }
    let log = EventLog::open(&path).await.unwrap();
    let changed = log
        .run_prefix("session", "source", Some(1), 1, bytes + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.events[0].event, first);
    assert_eq!(changed.high_water, before.high_water);
    assert_ne!(changed.digest, before.digest);

    // Synthetic/legacy duplicate Run identities must be unreadable, not latest-wins.
    let successor = log
        .run_boundary("session", "successor")
        .await
        .unwrap()
        .unwrap();
    append(
        &log,
        RuntimeEvent::new(
            successor.invocation,
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Completed,
            },
        ),
    )
    .await;
    append(
        &log,
        opening("session", "source", "ambiguous-invocation", "alias"),
    )
    .await;
    assert!(matches!(
        log.run_boundary("session", "source").await,
        Err(StoreError::InvalidTransition(_))
    ));
    assert!(matches!(
        log.run_prefix("session", "source", Some(1), 1, 16_384)
            .await,
        Err(StoreError::InvalidTransition(_))
    ));
    log.close().await.unwrap();
}
