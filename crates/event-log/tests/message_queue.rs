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
    message_resolution::MessageResolution,
};
use maka_runtime::{
    event::{EventWrite, Fact, InvocationOutcome, RuntimeEvent},
    message::{MessageDisposition as Disposition, Placement},
};
use serde_json::json;

#[path = "support/message_queue.rs"]
mod support;
use support::{admission, append, command, invocation, opening};

#[tokio::test]
async fn queue_edits_cancel_or_deliver_once_with_atomic_revision_and_original_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("queue.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let first = invocation("first");
    append(&log, &first, opening()).await;
    let mut notices = log.subscribe_commits();
    let before = *notices.borrow_and_update();
    let targets = ["session".into()];
    let version = log.observation_versions(&targets).await.unwrap()["session"];
    for id in ["one", "two", "three"] {
        log.admit_message(admission(&first, id, Disposition::Followup))
            .await
            .unwrap();
    }
    notices.changed().await.unwrap();
    assert_eq!(
        *notices.borrow_and_update(),
        before,
        "queue wakeup does not invent a log event"
    );
    let queue = log.message_queue("session").await.unwrap();
    assert_eq!(queue.revision, 3);
    let queued_version = log.observation_versions(&targets).await.unwrap()["session"];
    assert_eq!(queued_version.event, version.event);
    assert_eq!(
        queued_version.queue, queue.revision,
        "queue-only commits must invalidate observation"
    );
    let reordered = log
        .edit_message_queue(
            "session",
            3,
            QueueEdit::Reorder {
                message_ids: vec!["three".into(), "one".into(), "two".into()],
            },
            command("reorder", Kind::Reorder),
        )
        .await
        .unwrap();
    assert_eq!(reordered.revision, 4);
    let reordered = log.message_queue("session").await.unwrap();
    assert_eq!(
        reordered
            .entries
            .iter()
            .map(|e| e.source.message.message_id.as_str())
            .collect::<Vec<_>>(),
        ["three", "one", "two"]
    );
    assert!(matches!(
        log.edit_message_queue(
            "session",
            3,
            QueueEdit::RetractAll {
                cancellation_id: "stale".into()
            },
            command("stale", Kind::RetractAll),
        )
        .await,
        Err(StoreError::RevisionConflict { .. })
    ));
    assert_eq!(log.message_queue("session").await.unwrap(), reordered);
    let same = log
        .edit_message_queue(
            "session",
            4,
            QueueEdit::Reorder {
                message_ids: vec!["three".into(), "one".into(), "two".into()],
            },
            command("same", Kind::Reorder),
        )
        .await
        .unwrap();
    assert_eq!(same.revision, reordered.revision);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_cancel BEFORE INSERT ON message_cancellations
        WHEN NEW.message_id = 'one' BEGIN SELECT RAISE(ABORT, 'cancel fault'); END;",
    )
    .unwrap();
    assert!(
        log.edit_message_queue(
            "session",
            4,
            QueueEdit::RetractAll {
                cancellation_id: "fault".into()
            },
            command("fault", Kind::RetractAll),
        )
        .await
        .is_err()
    );
    assert_eq!(log.message_queue("session").await.unwrap(), reordered);
    for id in ["one", "two", "three"] {
        assert!(!log.message_cancelled("session", id).await.unwrap());
    }
    db.execute_batch("DROP TRIGGER reject_cancel").unwrap();
    let receipt: maka_runtime::input::InputReceipt = serde_json::from_value(json!({
        "source":{"kind":"input","name":"review","packageId":"reviewer","entryId":"entry","activation":"1","revision":"1"},
        "receipt":{"document":"report.md"}
    })).unwrap();
    let mut content: maka_runtime::input::MessageInput = "edited 😀".into();
    content.preparation.push(receipt.clone());
    let updated = log
        .edit_message_queue(
            "session",
            4,
            QueueEdit::Update {
                message_id: "two".into(),
                content: Box::new(content),
                required_tools: ["Read".into()].into(),
            },
            command("update", Kind::Update),
        )
        .await
        .unwrap();
    assert_eq!(updated.revision, 5);
    append(
        &log,
        &first,
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    )
    .await;
    let second = invocation("second");
    append(&log, &second, opening()).await;
    let promoted = log
        .edit_message_queue(
            "session",
            5,
            QueueEdit::Promote {
                message_id: "two".into(),
                invocation: second.clone(),
            },
            command("promote", Kind::Promote),
        )
        .await
        .unwrap();
    assert_eq!(promoted.revision, 6);
    let promoted = log.message_queue("session").await.unwrap();
    let entry = promoted
        .entries
        .iter()
        .find(|e| e.source.message.message_id == "two")
        .unwrap();
    assert_eq!(
        entry.invocation, first,
        "promotion does not rewrite original admission ownership"
    );
    assert_eq!(entry.steering_target(), &second);
    assert_eq!(entry.required_tools, ["Read".into()].into());
    assert_eq!(entry.source.submitted_placement, Placement::NextTurn);
    let mut omitted = entry.source.message.clone();
    omitted.content.preparation.clear();
    let omitted_receipt = EventWrite::plain(RuntimeEvent::new(
        second.clone(),
        Fact::MessageSteered {
            message: Box::new(omitted),
        },
    ))
    .unwrap();
    assert!(log.append(&omitted_receipt).await.is_err());
    assert_eq!(log.message_queue("session").await.unwrap(), promoted);
    assert_eq!(log.commit_pending_steering(&second).await.unwrap(), 1);
    assert!(matches!(
        log.edit_message_queue(
            "session",
            6,
            QueueEdit::Retract {
                message_id: "two".into(),
                cancellation_id: "late".into()
            },
            command("late", Kind::Retract),
        )
        .await,
        Err(StoreError::RevisionConflict { .. })
    ));
    let proof = log
        .steering_message("session", "two")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proof.event.invocation, second);
    assert!(
        matches!(&proof.event.fact, Fact::MessageSteered { message } if message.content.preparation == [receipt])
    );
    let queue = log.message_queue("session").await.unwrap();
    let cancelled = log
        .edit_message_queue(
            "session",
            queue.revision,
            QueueEdit::RetractAll {
                cancellation_id: "cancel-rest".into(),
            },
            command("cancel-rest", Kind::RetractAll),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.retracted.len(), 2);
    let queue = log.message_queue("session").await.unwrap();
    assert!(queue.entries.is_empty());
    for id in ["one", "three"] {
        assert!(log.message_cancelled("session", id).await.unwrap());
        assert!(
            log.admit_message(admission(&second, id, Disposition::Steering))
                .await
                .is_err()
        );
        assert!(
            log.append(
                &EventWrite::plain(RuntimeEvent::new(
                    second.clone(),
                    Fact::MessageSteered {
                        message: Box::new(
                            admission(&second, id, Disposition::Steering).source.message
                        ),
                    }
                ))
                .unwrap()
            )
            .await
            .is_err(),
            "cancelled identity cannot bypass the pending path"
        );
    }
    let ids: Vec<_> = ["one", "two", "three", "absent"].map(String::from).into();
    let resolutions = log.message_resolutions("session", &ids).await.unwrap();
    assert_eq!(
        resolutions,
        vec![
            MessageResolution::Cancelled {
                message_id: "one".into()
            },
            MessageResolution::Owned {
                message_id: "two".into(),
                invocation: second
            },
            MessageResolution::Cancelled {
                message_id: "three".into()
            },
        ]
    );
    assert!(
        log.message_resolutions("foreign", &ids)
            .await
            .unwrap()
            .is_empty()
    );
    let prefix = serde_json::to_vec(&log.prefix(100, 1024 * 1024).await.unwrap()).unwrap();
    log.close().await.unwrap();
    drop(db);
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(log.message_queue("session").await.unwrap(), queue);
    assert_eq!(
        log.message_resolutions("session", &ids).await.unwrap(),
        resolutions
    );
    assert_eq!(
        serde_json::to_vec(&log.prefix(100, 1024 * 1024).await.unwrap()).unwrap(),
        prefix
    );
    log.close().await.unwrap();
}
