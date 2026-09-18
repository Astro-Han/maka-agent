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
use maka_runtime::event::EventWrite;
use maka_runtime::event::{Fact, Invocation, InvocationInput, RuntimeEvent};
use rusqlite::Connection;
use serde_json::{Value, json};

async fn seed_session_and_event(log: &EventLog) -> RuntimeEvent {
    log.create_session("session", "request", &json!({"name":"kept"}), 10)
        .await
        .unwrap();
    let event = RuntimeEvent::new(
        Invocation {
            session_id: "session".into(),
            turn_id: "turn".into(),
            run_id: "run".into(),
            invocation_id: "invocation".into(),
        },
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                skill_invocation: Default::default(),
                source_messages: Vec::new(),
                content: "kept".into(),
                request_fingerprint: None,
            },
        },
    );
    log.append(&EventWrite::plain((event).clone()).unwrap())
        .await
        .unwrap();
    event
}

fn migration_checksums(connection: &Connection) -> Vec<(i64, Vec<u8>)> {
    connection
        .prepare(
            "SELECT version, checksum FROM _sqlx_migrations WHERE success = 1 ORDER BY version",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn embedded_migration_adopts_only_rust_schema_and_reopens_without_rewriting_facts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let event = seed_session_and_event(&log).await;
    log.append(
        &EventWrite::plain(RuntimeEvent::new(
            event.invocation.clone(),
            Fact::ToolDispatched {
                operation_id: "migration-tool".into(),
                call: maka_runtime::tool_call::ToolCallIdentity::standalone(
                    "migration-call".into(),
                ),
                name: "Read".into(),
                input: json!({}),
            },
        ))
        .unwrap(),
    )
    .await
    .unwrap();
    let (outcome, _) = EventWrite::tool_success(
        "migration-result".into(),
        event.recorded_at,
        event.invocation.clone(),
        "migration-tool".into(),
        maka_runtime::tool_output::ToolOutput::Text("payload survives parent migration".into())
            .into(),
    )
    .unwrap();
    log.append(&outcome).await.unwrap();
    let before = log.prefix(10, 16_384).await.unwrap();
    let session = log.get_session::<Value>("session").await.unwrap();
    assert!(log.prepare_transcript("session", 1, 32).await.unwrap());
    log.close().await.unwrap();
    let connection = Connection::open(&path).unwrap();
    let checksums = migration_checksums(&connection);
    let raw: Vec<u8> = connection
        .query_row(
            "SELECT payload FROM tool_result_payloads WHERE event_id = 'migration-result'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        checksums
            .iter()
            .map(|(version, _)| *version)
            .collect::<Vec<_>>(),
        vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26
        ]
    );
    assert!(checksums.iter().all(|(_, checksum)| checksum.len() == 48));
    let payload: Vec<u8> = connection
        .query_row("SELECT payload FROM transcript_rows", [], |row| row.get(0))
        .unwrap();
    // An epoch-151 display cache must not hide the new interruption semantics.
    // Only the disposable cache is invalidated; execution facts stay byte-exact.
    connection
        .execute_batch(
            "UPDATE transcript_rows SET payload = x'00', total_bytes = 1;
        DELETE FROM _sqlx_migrations WHERE version = 16; PRAGMA user_version = 15;",
        )
        .unwrap();
    drop(connection);
    let log = EventLog::open(&path).await.unwrap();
    assert!(log.prepare_transcript("session", 1, 32).await.unwrap());
    assert_eq!(
        serde_json::to_vec(&log.prefix(10, 16_384).await.unwrap()).unwrap(),
        serde_json::to_vec(&before).unwrap()
    );
    log.close().await.unwrap();
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT payload FROM transcript_rows", [], |row| row
                .get::<_, Vec<u8>>(0))
            .unwrap(),
        payload
    );
    assert_eq!(migration_checksums(&connection), checksums);
    // Restore the legacy version without a ledger; retaining the read-state
    // table also exercises idempotent backfill after interrupted adoption.
    connection
        .execute_batch(
            "DROP TABLE model_request_compositions; DROP TABLE request_compositions; DROP TABLE graph_wakes; DROP TABLE graph_intents; DROP TABLE graph_updates; DROP TABLE graph_epochs; DROP TABLE plugin_execution_receipts; DROP TABLE plugin_data; DROP TABLE plugin_packages; DROP TABLE plugin_package_files;
             DROP TABLE plugin_package_blobs; DROP TABLE plugin_composition;
             DROP VIEW workhub_corrections; DROP VIEW workhub_assignments;
             DROP VIEW workhub_stops; ALTER TABLE legacy_workhub_stops RENAME TO workhub_stops;
             DROP VIEW runtime_events; DROP VIEW session_events;
             ALTER TABLE event_log RENAME TO runtime_events;
             DROP TABLE workhub_stops; DROP INDEX workhub_action_identity; DROP TABLE project_locations; DROP TABLE project_identities; DROP TABLE projects; DROP INDEX continuation_claim_id; DROP INDEX continuation_source_boundary; DROP TABLE message_interrupt_receipts; DROP TABLE message_submit_receipts; DROP TABLE queue_command_receipts; DROP TABLE message_queue_state; DROP TABLE message_cancellations; DROP TABLE message_admissions; DROP TABLE _sqlx_migrations; PRAGMA user_version = 1;",
        )
        .unwrap();
    drop(connection);
    for _ in 0..2 {
        let log = EventLog::open(&path).await.unwrap();
        assert_eq!(
            serde_json::to_vec(&log.prefix(10, 16_384).await.unwrap()).unwrap(),
            serde_json::to_vec(&before).unwrap()
        );
        assert_eq!(
            log.probe_session_create::<Value>("session", "request")
                .await
                .unwrap(),
            session
        );
        assert_eq!(
            log.append(&EventWrite::plain((event).clone()).unwrap())
                .await
                .unwrap(),
            1
        );
        log.close().await.unwrap();
    }
    let connection = Connection::open(&path).unwrap();
    assert_eq!(migration_checksums(&connection), checksums);
    assert_eq!(
        connection
            .query_row(
                "SELECT payload FROM tool_result_payloads WHERE event_id = 'migration-result'",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap(),
        raw
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            },)
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM _sqlx_migrations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        26
    );
}

#[tokio::test]
async fn mismatched_and_unknown_migrations_fail_closed_without_touching_committed_events() {
    let temp = tempfile::tempdir().unwrap();
    for unknown in [false, true] {
        let path = temp.path().join(format!("events-{unknown}.sqlite"));
        let log = EventLog::open(&path).await.unwrap();
        let event = seed_session_and_event(&log).await;
        log.close().await.unwrap();
        let connection = Connection::open(&path).unwrap();
        let session_before: String = connection.query_row(
            "SELECT json_array(id, fingerprint, revision, created_at, updated_at, archived, configuration) FROM session_control",
            [], |row| row.get(0)
        ).unwrap();
        let sql = if unknown {
            "UPDATE _sqlx_migrations SET version = 99 WHERE version = 2"
        } else {
            "UPDATE _sqlx_migrations SET checksum = x'00' WHERE version = 1"
        };
        connection.execute_batch(sql).unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        let error = EventLog::open(&path).await.err().expect("must reject");
        assert!(
            std::fs::read(&path).unwrap() == before,
            "rejected open mutated database (unknown={unknown})"
        );
        if unknown {
            assert!(matches!(
                error,
                StoreError::Migration(sqlx::migrate::MigrateError::VersionMissing(99))
            ));
        } else {
            assert!(matches!(
                error,
                StoreError::Migration(sqlx::migrate::MigrateError::VersionMismatch(1))
            ));
        }
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM runtime_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let stored_event: String = connection
            .query_row(
                "SELECT event_json FROM runtime_events WHERE sequence = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&stored_event).unwrap(),
            serde_json::to_value(&event).unwrap()
        );
        assert_eq!(connection.query_row(
            "SELECT json_array(id, fingerprint, revision, created_at, updated_at, archived, configuration) FROM session_control",
            [], |row| row.get::<_, String>(0)
        ).unwrap(), session_before);
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM _sqlx_migrations", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            26
        );
    }

    let path = temp.path().join("foreign.sqlite");
    let foreign = Connection::open(&path).unwrap();
    foreign
        .execute_batch("CREATE TABLE user_data(value); INSERT INTO user_data VALUES('keep')")
        .unwrap();
    drop(foreign);
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        EventLog::open(&path).await,
        Err(StoreError::UnsupportedDatabase)
    ));
    assert!(std::fs::read(&path).unwrap() == before);
    let foreign = Connection::open(&path).unwrap();
    assert_eq!(
        foreign
            .query_row("SELECT value FROM user_data", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    assert_eq!(
        foreign
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = '_sqlx_migrations'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn interrupted_initial_migration_remains_openable_under_the_rust_application_identity() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let connection = Connection::open(&path).unwrap();
    // SQLx creates its ledger before applying migration 1. A crash at that
    // boundary must not misclassify the database as foreign on the next open.
    connection
        .execute_batch(
            "PRAGMA application_id = 1296124754;
         CREATE TABLE _sqlx_migrations (
            version BIGINT PRIMARY KEY, description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL
         );",
        )
        .unwrap();
    drop(connection);
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(log.prefix(1, 1024).await.unwrap().high_water, 0);
    log.close().await.unwrap();
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        26
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM _sqlx_migrations WHERE success",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        26
    );
}
