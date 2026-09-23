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
    let mut unprepared = content.clone();
    unprepared.text = format!("{turn} before preparation");
    let source = maka_runtime::message::RootSourceMessage {
        message: maka_runtime::input::DeliveredMessage {
            message_id: format!("message-{turn}"),
            content: content.clone(),
            submitted_content_digest: unprepared.content_digest().unwrap(),
        },
        unprepared_content: unprepared,
        submitted_placement: maka_runtime::message::Placement::CurrentTurn,
        disposition: maka_runtime::message::MessageDisposition::TurnStarted,
        submitted_intent: None,
    };
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
                source_messages: vec![source],
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
        purpose: maka_runtime::session::CopyPurpose::Branch {
            turn_id: Some("first".into()),
            side_conversation: false,
        },
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
            purpose: maka_runtime::session::CopyPurpose::Branch {
                turn_id: Some("first".into()),
                side_conversation: false,
            },
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
                purpose: maka_runtime::session::CopyPurpose::Branch {
                    turn_id: Some("first".into()),
                    side_conversation: true
                },
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
        let editable = log
            .editable_message(session, "first", "message-first")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(editable.content.text, "first before preparation");
        assert_eq!(
            editable.content.attachments.as_ref().unwrap()[0].storage_ref,
            StorageRef::SessionFile {
                session_id: session.into(),
                relative_path: files[0].id.clone()
            }
        );
        assert!(
            log.root_message(session, "message-first")
                .await
                .unwrap()
                .is_none(),
            "historical source access is not proof of target execution admission"
        );
        assert!(
            log.editable_message(session, "later", "message-later")
                .await
                .unwrap()
                .is_none(),
            "a source beyond the inherited cut is not editable in the copy"
        );
        assert!(
            log.editable_message(session, "first", "message-later")
                .await
                .unwrap()
                .is_none()
        );
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
        assert_eq!(
            log.editable_message(session, "first", "message-first")
                .await
                .unwrap()
                .unwrap(),
            editable
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
        purpose: maka_runtime::session::CopyPurpose::Branch {
            turn_id: None,
            side_conversation: false,
        },
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
    assert!(
        log.copy_session(
            SessionCopy {
                source_session_id: "source".into(),
                target_session_id: "target".into(),
                expected_source_revision: revision,
                purpose: maka_runtime::session::CopyPurpose::Revision {
                    turn_id: "missing-upload".into()
                },
            },
            &json!({}),
            3
        )
        .await
        .is_err(),
        "an excluded edit input still needs its attachments"
    );
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
            purpose: maka_runtime::session::CopyPurpose::EmptySideConversation,
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

#[tokio::test]
async fn revisions_own_excluded_inputs_and_allocate_one_durable_family() {
    use maka_runtime::session::{BranchOrigin, CopyPurpose, Lineage};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("revision.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("source", "source", &json!({}), 1)
        .await
        .unwrap();
    log.commit_artifact(
        Artifact {
            id: "upload".into(),
            session_id: "source".into(),
            turn_id: "first".into(),
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
    let source = log.get_session::<Value>("source").await.unwrap().unwrap();
    let request = SessionCopy {
        source_session_id: "source".into(),
        target_session_id: "revision-a".into(),
        expected_source_revision: source.revision,
        purpose: CopyPurpose::Revision {
            turn_id: "first".into(),
        },
    };
    let (a, b) = tokio::join!(
        copy(&log, request.clone()),
        copy(
            &log,
            SessionCopy {
                target_session_id: "revision-b".into(),
                ..request.clone()
            }
        )
    );
    let mut indexes = Vec::new();
    for record in [&a, &b] {
        let Some(Lineage::Revision {
            root_session_id,
            parent_session_id,
            turn_id,
            index,
            branch,
        }) = record.lineage.as_deref()
        else {
            panic!("revision has no lineage");
        };
        assert_eq!(
            (
                root_session_id.as_str(),
                parent_session_id.as_str(),
                turn_id.as_str()
            ),
            ("source", "source", "first")
        );
        assert!(branch.is_none());
        indexes.push(*index);
        assert!(record.execution.is_none());
        assert!(
            log.read_model_context(&record.id, None, 100, 65536)
                .await
                .unwrap()
                .tail
                .is_empty()
        );
        assert!(log.prepare_transcript(&record.id, 0, 32).await.unwrap());
        assert!(
            log.navigation_turns(&record.id, 0, 0, 32)
                .await
                .unwrap()
                .contributions
                .is_empty()
        );
        assert!(
            log.root_message(&record.id, "message-first")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            log.validate_message_identity(&record.id, "message-first")
                .await
                .is_err(),
            "editing cannot reuse the source's identity"
        );
        let mut collision = RuntimeEvent::new(
            Invocation {
                session_id: record.id.clone(),
                turn_id: "collision".into(),
                run_id: "collision".into(),
                invocation_id: "collision".into(),
            },
            Fact::InvocationOpened {
                configuration: None,
                input: InvocationInput::Message {
                    source_messages: Vec::new(),
                    content: "new execution".into(),
                    request_fingerprint: None,
                },
            },
        );
        collision.id = "message-first".into();
        assert!(
            matches!(
                log.append(&EventWrite::plain(collision).unwrap()).await,
                Err(maka_runtime::event::CommitError::Rejected(reason)) if reason == StoreError::EventConflict.to_string()
            ),
            "canonical events cannot take a retained source identity either"
        );
    }
    indexes.sort();
    assert_eq!(indexes, [2, 3]);
    let branched = copy(
        &log,
        SessionCopy {
            target_session_id: "branch".into(),
            purpose: CopyPurpose::Branch {
                turn_id: Some("first".into()),
                side_conversation: false,
            },
            ..request.clone()
        },
    )
    .await;
    let branch_revision = copy(
        &log,
        SessionCopy {
            source_session_id: branched.id,
            target_session_id: "branch-revision".into(),
            expected_source_revision: branched.revision,
            purpose: request.purpose.clone(),
        },
    )
    .await;
    assert_eq!(
        branch_revision.lineage.as_deref(),
        Some(&Lineage::Revision {
            root_session_id: "branch".into(),
            parent_session_id: "branch".into(),
            turn_id: "first".into(),
            index: 2,
            branch: Some(BranchOrigin {
                parent_session_id: "source".into(),
                turn_id: Some("first".into())
            }),
        })
    );
    turn(&log, "revision-a", "edited", false).await;
    let current = log
        .get_session::<Value>("revision-a")
        .await
        .unwrap()
        .unwrap();
    let next = copy(
        &log,
        SessionCopy {
            source_session_id: "revision-a".into(),
            target_session_id: "revision-next".into(),
            expected_source_revision: current.revision,
            purpose: CopyPurpose::Revision {
                turn_id: "edited".into(),
            },
        },
    )
    .await;
    assert_eq!(
        next.lineage.as_deref(),
        Some(&Lineage::Revision {
            root_session_id: "source".into(),
            parent_session_id: "revision-a".into(),
            turn_id: "edited".into(),
            index: 4,
            branch: None,
        })
    );
    log.delete_user_artifact("source", "upload").await.unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        copy(&log, request).await.lineage,
        a.lineage,
        "retries do not allocate another family index"
    );
    for target in ["revision-a", "revision-b", "branch-revision"] {
        let input = log
            .editable_message(target, "first", "message-first")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(input.content.text, "first before preparation");
        let StorageRef::SessionFile {
            session_id,
            relative_path,
        } = &input.content.attachments.as_ref().unwrap()[0].storage_ref
        else {
            panic!("editable input lost its retained upload");
        };
        assert_eq!(session_id, target);
        assert_eq!(
            log.read_artifact_chunk(target, relative_path, 0, 64)
                .await
                .unwrap()
                .unwrap()
                .bytes,
            b"hello"
        );
    }
    assert!(
        log.editable_message("revision-next", "first", "message-first")
            .await
            .unwrap()
            .is_none(),
        "another draft's excluded source is not inherited history"
    );
}
