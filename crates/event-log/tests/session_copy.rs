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

use maka_event_log::{
    EventLog, StoreError,
    artifacts::ArtifactDeletion,
    context::HistoryCut,
    sessions::{SessionCopy, SessionCopyResult},
};
use maka_runtime::{
    artifact::{Artifact, ArtifactKind, ArtifactSource, content_digest},
    attachment::{AttachmentKind, AttachmentRef, StorageRef},
    event::{
        EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, LogScope, RuntimeEvent,
    },
};
use serde_json::{Value, json};

fn attachment() -> AttachmentRef {
    AttachmentRef {
        kind: AttachmentKind::Other,
        name: "input.txt".into(),
        mime_type: "text/plain".into(),
        bytes: 5,
        storage_ref: StorageRef::SessionFile {
            session_id: "source".into(),
            relative_path: "upload".into(),
        },
    }
}

async fn turn(log: &EventLog, session: &str, turn: &str, file: bool) {
    let mut content: maka_runtime::input::MessageInput = turn.to_owned().into();
    if file {
        content.attachments = Some(vec![attachment()]);
    }
    let invocation = Invocation {
        session_id: session.into(),
        turn_id: turn.into(),
        run_id: format!("run-{turn}"),
        invocation_id: format!("inv-{turn}"),
    };
    for fact in [
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content,
                source_messages: Vec::new(),
                request_fingerprint: None,
            },
        },
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    ] {
        log.append(&EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap())
            .await
            .unwrap();
    }
}

async fn copy(
    log: &EventLog,
    request: SessionCopy,
) -> maka_event_log::sessions::SessionRecord<Value> {
    let SessionCopyResult::Committed(session) = log
        .copy_session(request, &json!({"name":"copy"}), 20)
        .await
        .unwrap()
    else {
        panic!("unchanged revision")
    };
    *session
}

#[tokio::test]
async fn copies_own_history_and_files_without_replaying_execution_across_retries_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("source", "source", &json!({}), 1)
        .await
        .unwrap();
    log.commit_artifact(
        Artifact {
            id: "upload".into(),
            session_id: "source".into(),
            turn_id: "upload".into(),
            created_at: 2,
            name: "input.txt".into(),
            kind: ArtifactKind::File,
            size_bytes: 5,
            mime_type: Some("text/plain".into()),
            source: ArtifactSource::UserUpload,
            summary: Some(content_digest(b"hello")),
        },
        b"hello",
    )
    .await
    .unwrap();
    turn(&log, "source", "first", true).await;
    let revision = log
        .get_session::<Value>("source")
        .await
        .unwrap()
        .unwrap()
        .revision;
    let request = SessionCopy {
        source_session_id: "source".into(),
        target_session_id: "branch".into(),
        expected_source_revision: revision,
        cut: HistoryCut::ThroughTurn("first".into()),
    };
    let first = copy(&log, request.clone()).await;
    assert!(
        first.execution.is_none(),
        "inherited facts are not target execution"
    );
    let root = log.prefix(100, 65536).await.unwrap();
    let inherited = log
        .scoped_prefix(
            LogScope::Session {
                id: "branch".into(),
            },
            100,
            65536,
        )
        .await
        .unwrap();
    assert_eq!(
        inherited
            .events
            .iter()
            .map(|e| (e.sequence, &e.event))
            .collect::<Vec<_>>(),
        root.events
            .iter()
            .map(|e| (e.sequence, &e.event))
            .collect::<Vec<_>>()
    );
    let context = log
        .read_model_context("branch", None, 100, 65536)
        .await
        .unwrap();
    assert_eq!(context.source_evidence.digest, inherited.digest);
    assert_eq!(context.tail.len(), 2);
    assert_eq!(
        log.read_frozen_model_context(&context.source_evidence, 100, 65536)
            .await
            .unwrap()
            .tail
            .len(),
        2
    );
    assert_eq!(
        log.delete_user_artifact("source", "upload").await.unwrap(),
        ArtifactDeletion::Deleted
    );
    let files = log.list_artifacts("branch", 0, 32).await.unwrap().records;
    assert_eq!(files.len(), 1);
    assert_eq!(
        log.delete_user_artifact("branch", &files[0].id)
            .await
            .unwrap(),
        ArtifactDeletion::Protected
    );
    // The original reference still occurs in canonical bytes. A descendant uses
    // the parent's ownership map, not the now-missing source upload.
    let nested = copy(
        &log,
        SessionCopy {
            source_session_id: "branch".into(),
            target_session_id: "nested".into(),
            expected_source_revision: first.revision,
            cut: HistoryCut::ThroughTurn("first".into()),
        },
    )
    .await;
    assert!(nested.execution.is_none());
    assert_eq!(
        log.list_artifacts("nested", 0, 32)
            .await
            .unwrap()
            .records
            .len(),
        1
    );
    turn(&log, "source", "later", false).await;
    assert_eq!(copy(&log, request.clone()).await, first);
    assert!(matches!(
        log.copy_session(
            SessionCopy {
                cut: HistoryCut::End,
                ..request.clone()
            },
            &json!({}),
            30
        )
        .await,
        Err(StoreError::SessionConflict)
    ));
    // Remove source catalog state, not retained canonical facts. History reads
    // and exact receipts must not rely on a live source Session.
    let inspect = rusqlite::Connection::open(&path).unwrap();
    inspect
        .execute_batch(
            "PRAGMA foreign_keys=ON;
        BEGIN;
        DELETE FROM artifact_catalog WHERE session_id='source';
        DELETE FROM session_read_state WHERE session_id='source';
        DELETE FROM session_control WHERE id='source';
        COMMIT;",
        )
        .unwrap();
    drop(inspect);
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(copy(&log, request).await, first);
    for session in ["branch", "nested"] {
        let prefix = log
            .scoped_prefix(LogScope::Session { id: session.into() }, 100, 65536)
            .await
            .unwrap();
        assert_eq!(
            prefix
                .events
                .iter()
                .map(|e| (e.sequence, &e.event))
                .collect::<Vec<_>>(),
            root.events
                .iter()
                .map(|e| (e.sequence, &e.event))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            log.read_model_context(session, None, 100, 65536)
                .await
                .unwrap()
                .tail
                .len(),
            2
        );
        let files = log.list_artifacts(session, 0, 32).await.unwrap().records;
        assert_eq!(
            log.read_artifact_chunk(session, &files[0].id, 0, 64)
                .await
                .unwrap()
                .unwrap()
                .bytes,
            b"hello"
        );
        let fence = log.navigation_fence(session).await.unwrap().unwrap();
        assert_eq!(
            fence,
            maka_presentation::watermark(root.high_water).unwrap()
        );
        assert!(
            log.prepare_transcript(session, root.high_water, 32)
                .await
                .unwrap()
        );
        let turns = log.navigation_turns(session, fence, 0, 32).await.unwrap();
        assert_eq!(turns.contributions.len(), 1);
        assert_eq!(turns.contributions[0].turn_id, "first");
        assert_eq!(
            log.navigation_landmarks(session, fence, 8, None)
                .await
                .unwrap()
                .len(),
            1
        );
        let history = log
            .history_text(session, root.high_water, None)
            .await
            .unwrap();
        let maka_plugins::session::history::Page::Ready { chunks, next, .. } = &history else {
            panic!("prepared history");
        };
        assert!(next.is_none());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "first\ninput.txt");
        assert_eq!(
            chunks[0].attachments[0].storage_ref,
            StorageRef::SessionFile {
                session_id: session.into(),
                relative_path: files[0].id.clone(),
            }
        );
        let inspect = rusqlite::Connection::open(&path).unwrap();
        for table in ["transcript_text", "transcript_rows", "transcript_progress"] {
            inspect
                .execute(
                    &format!("DELETE FROM {table} WHERE session_id = ?"),
                    [session],
                )
                .unwrap();
        }
        drop(inspect);
        assert!(
            log.prepare_transcript(session, root.high_water, 32)
                .await
                .unwrap()
        );
        assert_eq!(
            serde_json::to_value(
                log.history_text(session, root.high_water, None)
                    .await
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(history).unwrap(),
            "rebuilding cannot depend on source metadata or cache state"
        );
    }
    assert_eq!(
        log.prefix(100, 65536).await.unwrap().events.len(),
        4,
        "only real source work was appended"
    );
}

#[tokio::test]
async fn failed_copy_publishes_neither_destination_nor_partial_resources() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("source", "source", &json!({}), 1)
        .await
        .unwrap();
    turn(&log, "source", "missing-upload", true).await;
    let revision = log
        .get_session::<Value>("source")
        .await
        .unwrap()
        .unwrap()
        .revision;
    let request = SessionCopy {
        source_session_id: "source".into(),
        target_session_id: "target".into(),
        expected_source_revision: revision,
        cut: HistoryCut::End,
    };
    assert!(matches!(
        log.copy_session(
            SessionCopy {
                expected_source_revision: revision + 1,
                ..request.clone()
            },
            &json!({}),
            3
        )
        .await
        .unwrap(),
        SessionCopyResult::SourceRevisionConflict { .. }
    ));
    let catalog = log
        .list_sessions::<Value>(None, None, 32)
        .await
        .unwrap()
        .revision;
    assert!(matches!(log.copy_session(request, &json!({}), 3).await,
        Err(StoreError::InvalidTransition(message)) if message == "history artifact no longer exists"));
    assert!(log.get_session::<Value>("target").await.unwrap().is_none());
    assert_eq!(
        log.list_sessions::<Value>(None, None, 32)
            .await
            .unwrap()
            .revision,
        catalog
    );
    let empty = copy(
        &log,
        SessionCopy {
            source_session_id: "source".into(),
            target_session_id: "target".into(),
            expected_source_revision: revision,
            cut: HistoryCut::BeforeTurn("missing-upload".into()),
        },
    )
    .await;
    assert!(empty.execution.is_none());
    assert!(
        log.read_model_context("target", None, 100, 65536)
            .await
            .unwrap()
            .tail
            .is_empty()
    );
}
