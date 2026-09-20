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
use maka_runtime::event::{Fact, Invocation, InvocationOutcome, RuntimeEvent};
use serde_json::{Value, json};

#[tokio::test]
async fn create_replay_conflict_reopen_and_anchored_catalog_pages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let original = log
        .create_session("a", "request-a", &json!({"name": "A"}), 10)
        .await
        .unwrap();
    assert_eq!(
        original,
        log.create_session("a", "request-a", &json!({"name": "ignored"}), 20)
            .await
            .unwrap()
    );
    assert_eq!(
        Some(original.clone()),
        log.probe_session_create::<Value>("a", "request-a")
            .await
            .unwrap()
    );
    assert!(matches!(
        log.probe_session_create::<Value>("a", "other").await,
        Err(StoreError::SessionConflict)
    ));
    assert!(matches!(
        log.create_session("a", "other", &json!({}), 20).await,
        Err(StoreError::SessionConflict)
    ));
    for id in ["b", "c"] {
        log.create_session(id, id, &json!({"workspace":{"hostCwd":"/chosen"}}), 10)
            .await
            .unwrap();
    }
    let scoped = log
        .scoped_sessions::<Value>(
            maka_event_log::sessions::CatalogScope::Workspace("/chosen".into()),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        scoped
            .sessions
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
    let page = log.list_sessions::<Value>(None, None, 2).await.unwrap();
    assert_eq!(
        page.sessions
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(page.next_cursor.as_deref(), Some("b"));
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(Some(original), log.get_session::<Value>("a").await.unwrap());
    let rest = log
        .list_sessions::<Value>(Some(&page.revision), page.next_cursor.as_deref(), 2)
        .await
        .unwrap();
    assert_eq!(rest.sessions[0].id, "c");
    assert_eq!(rest.revision, page.revision);
    assert!(rest.next_cursor.is_none());
    log.create_session("d", "d", &json!({}), 30).await.unwrap();
    assert!(matches!(
        log.list_sessions::<Value>(Some(&page.revision), Some("b"), 2)
            .await,
        Err(StoreError::RevisionConflict { .. })
    ));
    let archived = log
        .set_session_archived::<Value>("a", true, 40)
        .await
        .unwrap();
    assert_eq!(
        (archived.revision, archived.created_at, archived.updated_at),
        (2, 10, 40)
    );
    assert!(
        log.scoped_sessions::<Value>(
            maka_event_log::sessions::CatalogScope::Session("a".into()),
            None,
            None,
        )
        .await
        .unwrap()
        .sessions
        .is_empty(),
        "read grants do not expose archived Sessions"
    );
    let before = log
        .list_sessions::<Value>(None, None, 32)
        .await
        .unwrap()
        .revision;
    assert_eq!(
        archived,
        log.set_session_archived::<Value>("a", true, 50)
            .await
            .unwrap()
    );
    assert_eq!(
        before,
        log.list_sessions::<Value>(None, None, 32)
            .await
            .unwrap()
            .revision
    );
    let active = log
        .set_session_archived::<Value>("a", false, 35)
        .await
        .unwrap();
    assert!(!active.archived);
    assert_eq!((active.revision, active.updated_at), (3, 40));
}

#[tokio::test]
async fn lifecycle_obeys_canonical_invocation_seal_and_validates_bounds() {
    let directory = tempfile::tempdir().unwrap();
    let log = EventLog::open(&directory.path().join("events.sqlite"))
        .await
        .unwrap();
    log.create_session("a", "a", &json!({}), 1).await.unwrap();
    log.create_session("b", "b", &json!({}), 1).await.unwrap();
    let invocation = Invocation {
        session_id: "a".into(),
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
                    input: maka_runtime::input::InvocationInput::Message {
                        source_messages: Vec::new(),
                        content: "".into(),
                        request_fingerprint: None,
                    },
                },
            ))
            .clone(),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(
        log.set_session_archived::<Value>("a", true, 2).await,
        Err(StoreError::SessionBusy)
    ));
    assert!(
        log.set_session_archived::<Value>("b", true, 2)
            .await
            .unwrap()
            .archived
    );
    log.append(
        &EventWrite::plain(
            (RuntimeEvent::new(
                invocation,
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
    assert!(
        log.set_session_archived::<Value>("a", true, 2)
            .await
            .unwrap()
            .archived
    );
    let archived_admission = RuntimeEvent::new(
        Invocation {
            session_id: "a".into(),
            turn_id: "late-turn".into(),
            run_id: "late-run".into(),
            invocation_id: "late-invocation".into(),
        },
        Fact::InvocationOpened {
            configuration: None,
            input: maka_runtime::input::InvocationInput::Message {
                source_messages: Vec::new(),
                content: "".into(),
                request_fingerprint: None,
            },
        },
    );
    assert!(
        log.append(&EventWrite::plain((archived_admission).clone()).unwrap())
            .await
            .is_err(),
        "archive and invocation admission serialize in the same transaction boundary"
    );
    assert!(matches!(
        log.set_session_archived::<Value>("missing", true, 2).await,
        Err(StoreError::SessionNotFound)
    ));
    assert!(
        log.create_session("bad/id", "a", &json!({}), 1)
            .await
            .is_err()
    );
    assert!(
        log.create_session("large", "large", &"x".repeat(65536), 1)
            .await
            .is_err()
    );
    assert!(
        log.create_session("time", "time", &json!({}), u64::MAX)
            .await
            .is_err()
    );
    assert!(log.list_sessions::<Value>(None, None, 33).await.is_err());
    assert!(
        log.list_sessions::<Value>(None, Some("a"), 1)
            .await
            .is_err()
    );
}
