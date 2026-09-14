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

use super::*;

async fn dispatch(log: &EventLog, step: &str) -> String {
    accepted(log, step, "Read").await;
    let operation = format!("{step}:reused-provider-id");
    log.append(&event(Fact::ToolDispatched {
        operation_id: operation.clone(),
        call: ToolCallIdentity::provider(step.into(), "reused-provider-id".into()),
        name: "Read".into(),
        input: json!({}),
    }))
    .await
    .unwrap();
    operation
}

async fn open(log: &EventLog) {
    log.append(&event(Fact::InvocationOpened {
        configuration: None,
        input: InvocationInput::Message {
            skill_invocation: Default::default(),
            source_messages: Vec::new(),
            content: "read".into(),
            request_fingerprint: None,
        },
    }))
    .await
    .unwrap();
}

#[tokio::test]
async fn ten_mib_raw_fragments_rebuild_after_restart_from_compact_facts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    open(&log).await;
    let operation = dispatch(&log, "read").await;
    let raw = json!({"kind":"text","text":"x".repeat(10 * 1024 * 1024)});
    let write = success(&operation, raw.clone());
    let through = log.append(&write).await.unwrap();
    let id = write.event().id.clone();
    drop(write);
    prepare(&log, through).await;
    assert!(log.prefix(100, 8 * 1024 * 1024).await.is_ok());
    let db = Connection::open(&path).unwrap();
    let original: Vec<u8> = db
        .query_row(
            "SELECT payload FROM transcript_rows WHERE message_id = ?",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    let mut assembled = Vec::new();
    while assembled.len() < original.len() {
        let length = (original.len() - assembled.len()).min(512 * 1024);
        let fragment = log
            .transcript_fragment(
                "session",
                through * 256,
                assembled.len() as u64,
                length as u64,
            )
            .await
            .unwrap();
        assembled.extend(fragment);
    }
    assert_eq!(assembled, original);
    assert_eq!(
        serde_json::from_slice::<Value>(&assembled).unwrap()["content"]["value"],
        raw
    );
    db.execute("DELETE FROM transcript_rows", []).unwrap();
    db.execute("DELETE FROM transcript_progress", []).unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.resolve_tool_result("session", &id)
            .await
            .unwrap()
            .to_json(),
        raw
    );
    prepare(&log, through).await;
    let rebuilt: Vec<u8> = db
        .query_row(
            "SELECT payload FROM transcript_rows WHERE message_id = ?",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rebuilt, original);
}

#[tokio::test]
async fn selected_boundary_does_not_hydrate_an_earlier_sibling_payload() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    open(&log).await;
    let operation = dispatch(&log, "sibling").await;
    let sibling = success(&operation, json!({"text":"x".repeat(12 * 1024 * 1024)}));
    let previous = log.append(&sibling).await.unwrap();
    prepare(&log, previous).await;
    let db = Connection::open(&path).unwrap();
    db.execute(
        "DELETE FROM tool_result_payloads WHERE event_id = ?",
        [&sibling.event().id],
    )
    .unwrap();
    let operation = dispatch(&log, "target").await;
    let target = success(&operation, json!({"text":"target"}));
    let through = log.append(&target).await.unwrap();
    prepare(&log, through).await;
    assert!(
        log.resolve_tool_result("session", &sibling.event().id)
            .await
            .is_err()
    );
    let value: Vec<u8> = db
        .query_row(
            "SELECT payload FROM transcript_rows WHERE message_id = ?",
            [&target.event().id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&value).unwrap()["content"]["value"]["text"],
        "target"
    );
}

#[tokio::test]
async fn escaped_raw_remains_durable_when_full_ui_row_exceeds_client_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    open(&log).await;
    let operation = dispatch(&log, "control").await;
    let raw = json!({"kind":"text","text":"\u{0000}".repeat(3 * 1024 * 1024)});
    let write = success(&operation, raw.clone());
    let through = log.append(&write).await.unwrap();
    assert!(matches!(
        log.prepare_transcript("session", through, 32).await,
        Err(maka_event_log::StoreError::Projection(
            maka_presentation::ProjectionError::TooLarge
        ))
    ));
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.resolve_tool_result("session", &write.event().id)
            .await
            .unwrap()
            .to_json(),
        raw
    );
    assert!(log.prefix(100, 8 * 1024 * 1024).await.is_ok());
    // A known-oversize UI row is rejected from compact evidence alone. Even
    // absent bytes must not trigger hydration; the raw resolver still detects
    // that damage, so this capacity decision does not certify payload integrity.
    let db = Connection::open(&path).unwrap();
    db.execute(
        "DELETE FROM tool_result_payloads WHERE event_id = ?",
        [&write.event().id],
    )
    .unwrap();
    assert!(matches!(
        log.prepare_transcript("session", through, 32).await,
        Err(maka_event_log::StoreError::Projection(
            maka_presentation::ProjectionError::TooLarge
        ))
    ));
    assert!(
        log.resolve_tool_result("session", &write.event().id)
            .await
            .is_err()
    );
}
