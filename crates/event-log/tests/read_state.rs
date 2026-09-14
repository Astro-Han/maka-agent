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

use maka_event_log::{EventLog, StoreError, sessions::SessionRecord};
use maka_runtime::event::EventWrite;
use maka_runtime::event::{Fact, Invocation, InvocationOutcome, RuntimeEvent};
use maka_runtime::input::InvocationInput;
use serde_json::{Value, json};

fn invocation(session: &str) -> Invocation {
    Invocation {
        session_id: session.into(),
        turn_id: format!("turn-{session}"),
        run_id: format!("run-{session}"),
        invocation_id: format!("invocation-{session}"),
    }
}

fn opening(session: &str) -> RuntimeEvent {
    RuntimeEvent::new(
        invocation(session),
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                skill_invocation: Default::default(),
                source_messages: Vec::new(),
                content: "visible".into(),
                request_fingerprint: None,
            },
        },
    )
}

async fn catalog(log: &EventLog) -> String {
    log.list_sessions::<Value>(None, None, 32)
        .await
        .unwrap()
        .revision
}

async fn acknowledge(log: &EventLog, message: &str) -> SessionRecord<Value> {
    log.set_session_read_marker::<Value>("session", message)
        .await
        .unwrap()
}

#[tokio::test]
async fn active_ack_finalization_replay_and_rebuild_preserve_durable_control() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let initial = log
        .create_session("session", "fingerprint", &json!({}), 10)
        .await
        .unwrap();
    assert!(!initial.read_state.has_unread);
    assert_eq!(initial.read_state.last_read_message_id, None);
    let opening = opening("session");
    log.append(&EventWrite::plain((opening).clone()).unwrap())
        .await
        .unwrap();
    let active = acknowledge(&log, &opening.id).await;
    assert_eq!(active.revision, 3);
    assert!(!active.read_state.has_unread);
    assert_eq!(
        active.read_state.last_read_message_id.as_deref(),
        Some(opening.id.as_str())
    );
    let terminal = RuntimeEvent::new(
        invocation("session"),
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    );
    log.append(&EventWrite::plain((terminal).clone()).unwrap())
        .await
        .unwrap();
    let finished = log.get_session::<Value>("session").await.unwrap().unwrap();
    assert_eq!(finished.revision, active.revision + 1);
    assert!(finished.read_state.has_unread);
    assert_eq!(
        finished.read_state.last_read_message_id,
        active.read_state.last_read_message_id
    );
    let bytes = serde_json::to_vec(&log.prefix(32, 65536).await.unwrap()).unwrap();
    let acknowledged = acknowledge(&log, &opening.id).await;
    assert_eq!(acknowledged.revision, finished.revision + 1);
    assert_eq!(acknowledged.execution, finished.execution);
    assert_eq!((acknowledged.created_at, acknowledged.updated_at), (10, 10));
    let revision = catalog(&log).await;
    log.append(&EventWrite::plain((terminal).clone()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        log.set_session_read_marker::<Value>("session", &opening.id)
            .await
            .unwrap(),
        acknowledged
    );
    assert_eq!(catalog(&log).await, revision);
    log.close().await.unwrap();
    let source = rusqlite::Connection::open(&path).unwrap();
    source
        .execute_batch("DROP TABLE catalog_messages;")
        .unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.probe_session_create::<Value>("session", "fingerprint")
            .await
            .unwrap(),
        Some(acknowledged)
    );
    assert_eq!(catalog(&log).await, revision);
    assert_eq!(
        serde_json::to_vec(&log.prefix(32, 65536).await.unwrap()).unwrap(),
        bytes
    );
}

#[tokio::test]
async fn unknown_and_empty_tail_ids_leave_all_authorities_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let log = EventLog::open(&temp.path().join("events.sqlite"))
        .await
        .unwrap();
    log.create_session("session", "fingerprint", &json!({}), 7)
        .await
        .unwrap();
    log.create_session("other", "other", &json!({}), 7)
        .await
        .unwrap();
    for populated in [false, true] {
        let other = opening("other");
        if populated {
            log.append(&EventWrite::plain((opening("session")).clone()).unwrap())
                .await
                .unwrap();
            log.append(&EventWrite::plain((other).clone()).unwrap())
                .await
                .unwrap();
        }
        let before = log.get_session::<Value>("session").await.unwrap().unwrap();
        let revision = catalog(&log).await;
        let bytes = serde_json::to_vec(&log.prefix(32, 65536).await.unwrap()).unwrap();
        for id in ["unknown", "invocation-session", other.id.as_str()] {
            let record = log
                .set_session_read_marker::<Value>("session", id)
                .await
                .unwrap();
            assert_eq!(record, before);
        }
        assert_eq!(catalog(&log).await, revision);
        assert_eq!(
            serde_json::to_vec(&log.prefix(32, 65536).await.unwrap()).unwrap(),
            bytes
        );
    }
    assert!(matches!(
        log.set_session_read_marker::<Value>("missing", "unknown")
            .await,
        Err(StoreError::SessionNotFound)
    ));
    for invalid in ["", "invalid:id", "with space"] {
        assert!(matches!(
            log.set_session_read_marker::<Value>("session", invalid)
                .await,
            Err(StoreError::InvalidTransition(_))
        ));
    }
}

#[tokio::test]
async fn migration_backfills_only_existing_sessions_and_never_replays_over_acknowledgement() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    for id in ["session", "empty"] {
        log.create_session(id, id, &json!({}), 1).await.unwrap();
    }
    let visible = opening("session");
    log.append(&EventWrite::plain((visible).clone()).unwrap())
        .await
        .unwrap();
    log.append(
        &EventWrite::plain(
            (RuntimeEvent::new(
                invocation("session"),
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
    // Standalone event streams have no control row; finalization still succeeds.
    log.append(&EventWrite::plain((opening("standalone")).clone()).unwrap())
        .await
        .unwrap();
    log.append(
        &EventWrite::plain(
            (RuntimeEvent::new(
                invocation("standalone"),
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
    let bytes = serde_json::to_vec(&log.prefix(32, 65536).await.unwrap()).unwrap();
    let revision = catalog(&log).await;
    log.close().await.unwrap();
    let source = rusqlite::Connection::open(&path).unwrap();
    source
        .execute_batch(
            "DROP TABLE session_read_state; DELETE FROM _sqlx_migrations WHERE version = 2;
         PRAGMA user_version = 1;",
        )
        .unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert!(
        log.get_session::<Value>("session")
            .await
            .unwrap()
            .unwrap()
            .read_state
            .has_unread
    );
    assert!(
        !log.get_session::<Value>("empty")
            .await
            .unwrap()
            .unwrap()
            .read_state
            .has_unread
    );
    assert_eq!(catalog(&log).await, revision);
    let ack = acknowledge(&log, &visible.id).await;
    log.close().await.unwrap();
    // Legacy ledger adoption must also preserve an already durable acknowledgement.
    source
        .execute_batch("DROP TABLE project_locations; DROP TABLE project_identities; DROP TABLE projects; DROP INDEX continuation_claim_id; DROP INDEX continuation_source_boundary; DROP TABLE message_interrupt_receipts; DROP TABLE message_submit_receipts; DROP TABLE queue_command_receipts; DROP TABLE message_queue_state; DROP TABLE message_cancellations; DROP TABLE message_admissions; DROP TABLE _sqlx_migrations;")
        .unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.get_session::<Value>("session").await.unwrap(),
        Some(ack)
    );
    assert_eq!(
        serde_json::to_vec(&log.prefix(32, 65536).await.unwrap()).unwrap(),
        bytes
    );
}
