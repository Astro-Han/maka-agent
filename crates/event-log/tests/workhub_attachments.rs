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
use maka_runtime::{
    artifact::{Artifact, ArtifactKind, ArtifactSource, content_digest},
    attachment::{AttachmentKind, AttachmentRef, StorageRef},
    event::{EventWrite, Fact, Invocation, InvocationInput, RuntimeEvent},
    input::MessageInput,
    workhub::{COORDINATION_SESSION_ID, Delegation},
};
use serde_json::json;

fn upload(id: &str) -> Artifact {
    Artifact {
        id: id.into(),
        session_id: COORDINATION_SESSION_ID.into(),
        turn_id: "upload".into(),
        created_at: 1,
        name: format!("{id}.txt"),
        kind: ArtifactKind::File,
        size_bytes: 65_536,
        mime_type: Some("text/plain".into()),
        source: ArtifactSource::UserUpload,
        summary: Some(content_digest(&vec![b'x'; 65_536])),
    }
}
fn attachment(artifact: &Artifact) -> AttachmentRef {
    AttachmentRef {
        kind: AttachmentKind::Other,
        name: artifact.name.clone(),
        mime_type: artifact.mime_type.clone().unwrap(),
        bytes: artifact.size_bytes,
        storage_ref: StorageRef::SessionFile {
            session_id: artifact.session_id.clone(),
            relative_path: artifact.id.clone(),
        },
    }
}

#[tokio::test]
async fn delegation_copies_attachments_atomically_and_replays_without_the_deleted_source() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    for session in [COORDINATION_SESSION_ID, "target"] {
        log.create_session(session, "create", &json!({}), 1)
            .await
            .unwrap();
    }
    let first = upload("first");
    let second = upload("second");
    log.commit_artifact(first.clone(), vec![b'x'; 65_536])
        .await
        .unwrap();
    log.commit_artifact(second.clone(), vec![b'x'; 65_536])
        .await
        .unwrap();
    let coordinator = Invocation {
        session_id: COORDINATION_SESSION_ID.into(),
        turn_id: "source-turn".into(),
        run_id: "source-run".into(),
        invocation_id: "source-invocation".into(),
    };
    let content = MessageInput {
        attachments: Some(vec![attachment(&first), attachment(&second)]),
        .."Use both attachments".into()
    };
    let opening = EventWrite::plain(RuntimeEvent::new(
        coordinator.clone(),
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: content.clone(),
                request_fingerprint: None,
                source_messages: Vec::new(),
                skill_invocation: None,
            },
        },
    ))
    .unwrap();
    log.append(&opening).await.unwrap();
    log.delete_user_artifact(COORDINATION_SESSION_ID, &second.id)
        .await
        .unwrap();
    let delegation = Delegation {
        kind: Default::default(),
        action_id: "copy-action".into(),
        request_fingerprint: content_digest(b"copy request"),
        source_message_event_id: opening.event().id.clone(),
        target_revision: 1,
        target: Invocation {
            session_id: "target".into(),
            turn_id: "target-turn".into(),
            run_id: "target-run".into(),
            invocation_id: "target-invocation".into(),
        },
        delegation_text: "Read the evidence".into(),
    };
    let target_message = delegation.message(&content).unwrap();
    let action = EventWrite::plain(RuntimeEvent::new(
        coordinator,
        Fact::WorkhubDelegated {
            delegation: Box::new(delegation),
        },
    ))
    .unwrap();
    let initial = log.list_artifacts("target", 0, 128).await.unwrap().revision;
    assert!(
        log.append(&action).await.is_err(),
        "the second source is missing"
    );
    assert_eq!(
        log.list_artifacts("target", 0, 128).await.unwrap().revision,
        initial
    );
    assert!(
        log.list_artifacts("target", 0, 128)
            .await
            .unwrap()
            .records
            .is_empty()
    );
    assert!(log.pending_messages("target").await.unwrap().is_empty());
    log.commit_artifact(second.clone(), vec![b'x'; 65_536])
        .await
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_copy_action BEFORE INSERT ON runtime_events
        WHEN NEW.kind = 'workhub_delegated' BEGIN SELECT RAISE(ABORT, 'injected action failure'); END;").unwrap();
    assert!(log.append(&action).await.is_err());
    assert_eq!(
        log.list_artifacts("target", 0, 128).await.unwrap().revision,
        initial
    );
    assert!(
        log.list_artifacts("target", 0, 128)
            .await
            .unwrap()
            .records
            .is_empty()
    );
    assert!(log.pending_messages("target").await.unwrap().is_empty());
    assert!(log.workhub_action("copy-action").await.unwrap().is_none());
    db.execute_batch("DROP TRIGGER fail_copy_action;").unwrap();
    drop(db);
    let sequence = log.append(&action).await.unwrap();
    assert_eq!(
        log.pending_messages("target").await.unwrap()[0].source,
        target_message
    );
    let records = log.list_artifacts("target", 0, 128).await.unwrap().records;
    assert_eq!(records.len(), 2);
    for copied in &records {
        assert_eq!(copied.session_id, "target");
        assert_eq!(copied.turn_id, "target-turn");
        assert!(copied.id != first.id && copied.id != second.id);
        assert_eq!(
            log.read_artifact_chunk("target", &copied.id, 0, 65_536)
                .await
                .unwrap()
                .unwrap()
                .bytes,
            vec![b'x'; 65_536]
        );
    }
    for source in [&first, &second] {
        log.delete_user_artifact(COORDINATION_SESSION_ID, &source.id)
            .await
            .unwrap();
    }
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(log.append(&action).await.unwrap(), sequence);
    assert_eq!(
        log.list_artifacts("target", 0, 128).await.unwrap().records,
        records
    );
    assert_eq!(
        log.pending_messages("target").await.unwrap()[0].source,
        target_message
    );
    log.close().await.unwrap();
}
