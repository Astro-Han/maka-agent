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
    message_queue::{QueueCommandKind as Kind, QueueEdit},
};
use maka_runtime::{
    event::{Fact, InvocationInput, InvocationOutcome},
    message::MessageDisposition as Disposition,
};
use serde_json::json;

#[path = "support/message_queue.rs"]
mod support;
use support::{admission, append, command, invocation, opening};

#[tokio::test]
async fn submit_receipt_is_atomic_and_survives_edit_delivery_cancellation_and_epoch_change() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("submit.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let owner = invocation("original");
    append(&log, &owner, opening()).await;
    let mut pending = admission(&owner, "queued", Disposition::Followup);
    pending.required_tools.insert("Bash".into());
    pending
        .source
        .skill_invocation
        .loaded
        .push(maka_runtime::skills::LoadedSkill {
            id: "shell-work".into(),
            name: "Shell work".into(),
        });
    let before = log.message_queue("session").await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_submit BEFORE INSERT ON message_submit_receipts
         BEGIN SELECT RAISE(ABORT, 'submit receipt fault'); END;",
    )
    .unwrap();
    assert!(
        log.admit_queued_message("epoch", before.revision, pending.clone())
            .await
            .is_err()
    );
    assert_eq!(log.message_queue("session").await.unwrap(), before);
    assert!(
        log.message_submit_receipt("epoch", "session", "queued")
            .await
            .unwrap()
            .is_none()
    );
    db.execute_batch("DROP TRIGGER reject_submit").unwrap();
    let receipt = log
        .admit_queued_message("epoch", before.revision, pending.clone())
        .await
        .unwrap();
    assert_eq!(receipt.queue_revision, before.revision + 1);
    let updated = log
        .edit_message_queue(
            "session",
            receipt.queue_revision,
            QueueEdit::Update {
                message_id: "queued".into(),
                content: Box::new("edited".into()),
                skill_invocation: Default::default(),
                required_tools: Default::default(),
            },
            command("edit", Kind::Update),
        )
        .await
        .unwrap();
    assert!(
        log.pending_messages("session").await.unwrap()[0]
            .required_tools
            .is_empty(),
        "plain editing removes the old Skills prerequisites atomically"
    );
    assert_eq!(
        log.admit_queued_message("epoch", before.revision, pending.clone())
            .await
            .unwrap(),
        receipt
    );
    let mut conflict = pending.clone();
    conflict.source.message.submitted_content_digest = format!("sha256:{}", "e".repeat(64));
    assert!(matches!(
        log.admit_queued_message("epoch", updated.revision, conflict)
            .await,
        Err(StoreError::InvalidTransition(_))
    ));
    let edited = log
        .message_admission("session", "queued")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(edited.source.message.content.text, "edited");
    assert_eq!(
        edited.source.message.submitted_content_digest,
        pending.source.message.submitted_content_digest,
        "queue edits preserve canonical proof of the original submission"
    );
    append(
        &log,
        &owner,
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    )
    .await;
    // The still-owned cleanup interval permits a followup reservation, whereas
    // the raw API remains restricted to an unsealed Run.
    let late = admission(&owner, "late", Disposition::Followup);
    assert!(log.admit_message(late.clone()).await.is_err());
    let late_receipt = log
        .admit_queued_message("epoch", updated.revision, late.clone())
        .await
        .unwrap();
    let successor = invocation("successor");
    append(
        &log,
        &successor,
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: edited.source.message.content.clone(),
                request_fingerprint: None,
                skill_invocation: Default::default(),
                source_messages: vec![edited.source],
            },
        },
    )
    .await;
    assert!(
        log.message_admission("session", "queued")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        log.message_submit_receipt("epoch", "session", "queued")
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    let revision = log.message_queue("session").await.unwrap().revision;
    log.edit_message_queue(
        "session",
        revision,
        QueueEdit::RetractAll {
            cancellation_id: "cancel".into(),
        },
        command("cancel", Kind::RetractAll),
    )
    .await
    .unwrap();
    assert!(log.message_cancelled("session", "late").await.unwrap());
    assert_eq!(
        log.admit_queued_message("epoch", 0, late).await.unwrap(),
        late_receipt
    );
    let prefix = serde_json::to_vec(&log.prefix(100, 1024 * 1024).await.unwrap()).unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.message_submit_receipt("epoch", "session", "queued")
            .await
            .unwrap(),
        Some(receipt)
    );
    assert!(
        log.root_message("session", "queued")
            .await
            .unwrap()
            .is_some()
    );
    log.begin_message_epoch("new-epoch").await.unwrap();
    assert!(
        log.message_submit_receipt("epoch", "session", "queued")
            .await
            .unwrap()
            .is_none()
    );
    assert!(log.message_cancelled("session", "late").await.unwrap());
    assert_eq!(
        serde_json::to_vec(&log.prefix(100, 1024 * 1024).await.unwrap()).unwrap(),
        prefix
    );
    log.close().await.unwrap();
}
