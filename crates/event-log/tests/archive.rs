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
    context::{ContextEvent, HistoryCapture, HistoryCut, LatestMainContext, ModelContextSource},
};
use maka_runtime::{
    archive::{ArchivedPlaceholder, outcome_projection},
    context::{CheckpointMode, CompactOutcome, ContextCheckpoint, ModelPurpose, TextSummary},
    event::{
        EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, LogScope, RuntimeEvent,
    },
    model::ModelStep,
    tool_call::ToolCallIdentity,
    tool_output::ToolSuccess,
};
use serde_json::json;
#[path = "archive/fixtures.rs"]
mod fixtures;
#[path = "archive/lineage.rs"]
mod lineage;
use fixtures::*;

#[tokio::test]
async fn archive_atomic_retry_reopen_scope_and_source_integrity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("archive.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "history-source", &json!({}), 1)
        .await
        .unwrap();
    open(&log, "old", false).await;
    let target = tool(&log, "old", "step", "x".repeat(12_000)).await;
    let unpruned = log
        .read_tool_result("session", &target.event().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unpruned.tool_name, "Read");
    assert_eq!(
        unpruned.serialized_result,
        serde_json::to_string(&"x".repeat(12_000)).unwrap()
    );
    assert!(
        log.read_tool_result("other", &target.event().id)
            .await
            .unwrap()
            .is_none()
    );
    end(&log, "old").await;
    let frozen = log
        .read_model_context("session", None, 100, 64 * 1024)
        .await
        .unwrap();
    let initial_revision = log
        .get_session::<serde_json::Value>("session")
        .await
        .unwrap()
        .unwrap()
        .revision;
    let HistoryCapture::Captured(before_archive) = log
        .capture_session_history(
            "session",
            initial_revision,
            HistoryCut::ThroughTurn("old".into()),
            100,
            64 * 1024,
        )
        .await
        .unwrap()
    else {
        panic!("unchanged source");
    };
    use maka_event_log::sessions::{SessionCopy, SessionCopyResult};
    assert!(matches!(
        log.copy_session(
            SessionCopy {
                source_session_id: "session".into(),
                target_session_id: "unpruned-copy".into(),
                expected_source_revision: initial_revision,
                cut: HistoryCut::ThroughTurn("old".into()),
            },
            &json!({}),
            2
        )
        .await
        .unwrap(),
        SessionCopyResult::Committed(_)
    ));
    open(&log, "writer", false).await;
    let source = log
        .read_model_context("session", Some("writer"), 100, 64 * 1024)
        .await
        .unwrap();
    let mut main = request("writer", "writer-main", None).event().clone();
    if let Fact::ModelRequested {
        source_high_water,
        source_digest,
        ..
    } = &mut main.fact
    {
        *source_high_water = source.source_evidence.high_water;
        *source_digest = source.source_evidence.digest;
    }
    log.append(&EventWrite::plain(main).unwrap()).await.unwrap();
    log.append(&event(
        "writer",
        Fact::ModelCompleted {
            step_id: "writer-main".into(),
            output: serde_json::from_value(
                json!({"parts":[],"finish_reason":"stop","usage":{"input_tokens":10}}),
            )
            .unwrap(),
        },
    ))
    .await
    .unwrap();
    let write = archive("writer", &target);
    let Fact::ToolResultArchived { placeholder } = &write.event().fact else {
        panic!()
    };
    assert!(
        log.read_archive("session", &placeholder.identity)
            .await
            .unwrap()
            .is_none()
    );
    let mut forged_page = write.event().clone();
    if let Fact::ToolResultArchived { placeholder } = &mut forged_page.fact {
        placeholder.page.content.replace_range(..1, "y");
    }
    let commits = log.subscribe_commits();
    assert!(
        log.append(&EventWrite::plain(forged_page).unwrap())
            .await
            .is_err()
    );
    assert!(!commits.has_changed().unwrap());
    let inspect = rusqlite::Connection::open(&path).unwrap();
    inspect.execute_batch("CREATE TRIGGER reject_archive BEFORE INSERT ON event_log WHEN NEW.kind='tool_result_archived' BEGIN SELECT RAISE(ABORT,'injected archive failure'); END;").unwrap();
    let commits = log.subscribe_commits();
    assert!(log.append(&write).await.is_err());
    assert!(!commits.has_changed().unwrap());
    assert!(
        log.read_archive("session", &placeholder.identity)
            .await
            .unwrap()
            .is_none()
    );
    inspect
        .execute_batch("DROP TRIGGER reject_archive")
        .unwrap();
    log.append(&write).await.unwrap();
    assert!(
        matches!(log.latest_main_context("session").await.unwrap(),LatestMainContext::Selected(selected) if !selected.projection_current)
    );
    let original = log
        .read_frozen_model_context(&frozen.source_evidence, 100, 64 * 1024)
        .await
        .unwrap();
    assert_eq!(
        original.effective_source_digest,
        frozen.effective_source_digest
    );
    assert!(original.tail.iter().any(
        |entry| matches!(entry, ContextEvent::Canonical(entry) if entry.event.id == target.event().id)
    ));
    assert!(
        matches!(
            log.read_frozen_model_context(&frozen.source_evidence, 100, 8192)
                .await,
            Err(StoreError::PrefixTooLarge)
        ),
        "later archiving must not shrink a frozen source's original projection"
    );
    let commits = log.subscribe_commits();
    log.append(&write).await.unwrap();
    assert!(!commits.has_changed().unwrap());
    let source = log
        .read_model_context("session", Some("writer"), 100, 16 * 1024)
        .await
        .unwrap();
    assert!(
        source
            .tail
            .iter()
            .any(|e| matches!(e,ContextEvent::Archived(a) if a.event_id==target.event().id))
    );
    assert!(
        log.read_archive("other", &placeholder.identity)
            .await
            .unwrap()
            .is_none()
    );
    let mut forged = placeholder.identity.clone();
    forged.body_sha256 = "0".repeat(64);
    assert!(log.read_archive("session", &forged).await.is_err());
    let expected = log
        .read_archive("session", &placeholder.identity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        log.read_tool_result("session", &target.event().id)
            .await
            .unwrap()
            .unwrap()
            .serialized_result
            .as_bytes(),
        expected
    );
    assert_eq!(expected, serde_json::to_vec(&"x".repeat(12_000)).unwrap());
    end(&log, "writer").await;
    let final_revision = log
        .get_session::<serde_json::Value>("session")
        .await
        .unwrap()
        .unwrap()
        .revision;
    assert!(matches!(
        log.copy_session(
            SessionCopy {
                source_session_id: "session".into(),
                target_session_id: "archived-copy".into(),
                expected_source_revision: final_revision,
                cut: HistoryCut::ThroughTurn("old".into()),
            },
            &json!({}),
            3
        )
        .await
        .unwrap(),
        SessionCopyResult::Committed(_)
    ));
    let adopted = log
        .read_model_context("archived-copy", None, 100, 64 * 1024)
        .await
        .unwrap();
    assert_eq!(
        log.read_archive("archived-copy", &placeholder.identity)
            .await
            .unwrap()
            .unwrap(),
        expected,
        "copy reads its accepted archive without borrowing source Session access"
    );
    assert!(
        log.read_archive("unpruned-copy", &placeholder.identity)
            .await
            .unwrap()
            .is_none(),
        "a later source archive does not become an accepted archive in an older copy"
    );
    for session in ["archived-copy", "unpruned-copy"] {
        assert_eq!(
            log.read_tool_result(session, &target.event().id)
                .await
                .unwrap()
                .unwrap()
                .serialized_result
                .as_bytes(),
            expected
        );
    }
    assert!(
        adopted
            .tail
            .iter()
            .any(|e| matches!(e, ContextEvent::Archived(a) if a.event_id == target.event().id))
    );
    assert_eq!(
        log.read_frozen_model_context(&adopted.source_evidence, 100, 64 * 1024)
            .await
            .unwrap()
            .effective_source_digest,
        adopted.effective_source_digest
    );
    assert!(
        matches!(
            log.read_model_context("unpruned-copy", None, 100, 8192)
                .await,
            Err(StoreError::PrefixTooLarge)
        ),
        "later source pruning cannot replace history already owned by a copy"
    );
    let copy_invocation = Invocation {
        session_id: "archived-copy".into(),
        turn_id: "copy-turn".into(),
        run_id: "copy-run".into(),
        invocation_id: "copy-inv".into(),
    };
    for fact in [
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: "continue".into(),
                source_messages: Vec::new(),
                request_fingerprint: None,
            },
        },
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    ] {
        log.append(&EventWrite::plain(RuntimeEvent::new(copy_invocation.clone(), fact)).unwrap())
            .await
            .unwrap();
    }
    assert_eq!(
        log.context_before_run(&copy_invocation, 100, 64 * 1024)
            .await
            .unwrap()
            .effective_source_digest,
        adopted.effective_source_digest
    );
    let lineage = log
        .scoped_prefix(
            LogScope::Lineage {
                session_id: "archived-copy".into(),
                run_id: "copy-run".into(),
            },
            100,
            65536,
        )
        .await
        .unwrap();
    let lineage = log
        .read_frozen_model_context(
            &maka_event_log::context::SourceEvidence {
                scope: lineage.scope,
                high_water: lineage.high_water,
                digest: lineage.digest,
            },
            100,
            64 * 1024,
        )
        .await
        .unwrap();
    assert!(
        lineage
            .tail
            .iter()
            .any(|e| matches!(e, ContextEvent::Archived(a) if a.event_id == target.event().id)),
        "adopted archive writer need not belong to the new Run's lineage"
    );
    assert!(matches!(log.capture_session_history(
        "session", initial_revision, HistoryCut::End, 100, 8192,
    ).await.unwrap(), HistoryCapture::SourceRevisionConflict { expected, actual }
        if expected == initial_revision && actual == final_revision));
    let HistoryCapture::Captured(after_archive) = log
        .capture_session_history(
            "session",
            final_revision,
            HistoryCut::ThroughTurn("old".into()),
            100,
            16 * 1024,
        )
        .await
        .unwrap()
    else {
        panic!("unchanged source");
    };
    // The retained Turn is unchanged, but an already accepted later archive
    // belongs to the copy's frozen rendering. It is not a copied writer Run.
    assert_eq!(
        after_archive.context.source_evidence.digest,
        before_archive.context.source_evidence.digest
    );
    assert_ne!(
        after_archive.context.effective_source_digest,
        before_archive.context.effective_source_digest
    );
    assert!(after_archive.observed_through > before_archive.observed_through);
    assert!(after_archive.context.tail.iter().any(
        |entry| matches!(entry, ContextEvent::Archived(entry) if entry.event_id == target.event().id)
    ));
    assert!(after_archive.context.tail.iter().all(|entry| match entry {
        ContextEvent::Canonical(entry) => entry.event.invocation.invocation_id == "old",
        ContextEvent::Archived(entry) => entry.invocation.invocation_id == "old",
    }));
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    let HistoryCapture::Captured(reopened) = log
        .capture_session_history(
            "session",
            final_revision,
            HistoryCut::ThroughTurn("old".into()),
            100,
            16 * 1024,
        )
        .await
        .unwrap()
    else {
        panic!("unchanged source after reopen");
    };
    assert_eq!(
        reopened.context.effective_source_digest,
        after_archive.context.effective_source_digest
    );
    log.append(&write).await.unwrap();
    assert_eq!(
        log.read_archive("session", &placeholder.identity)
            .await
            .unwrap()
            .unwrap(),
        expected
    );
    inspect.execute("UPDATE event_log SET event_json=replace(event_json,'xxxxxxxx','yyyyyyyy') WHERE event_id=?", [&target.event().id]).unwrap();
    assert!(
        log.read_tool_result("session", &target.event().id)
            .await
            .is_err()
    );
    assert!(
        log.read_archive("archived-copy", &placeholder.identity)
            .await
            .is_err(),
        "history membership never bypasses original evidence integrity"
    );
    assert!(
        log.read_archive("session", &placeholder.identity)
            .await
            .is_err()
    );
    assert!(
        log.read_model_context("session", None, 100, 8192)
            .await
            .is_err()
    );
    log.close().await.unwrap();
}

#[tokio::test]
async fn bounded_prune_candidates_do_not_materialize_the_old_tail() {
    let directory = tempfile::tempdir().unwrap();
    let log = EventLog::open(&directory.path().join("large.sqlite"))
        .await
        .unwrap();
    open(&log, "old", false).await;
    for n in 0..40 {
        tool(&log, "old", &format!("step-{n}"), "x".repeat(220_000)).await;
    }
    end(&log, "old").await;
    open(&log, "recent", false).await;
    end(&log, "recent").await;
    open(&log, "current", false).await;
    tool(&log, "current", "current-result", "small".into()).await;
    assert!(matches!(
        log.read_model_context("session", Some("current"), 10_000, 8 * 1024 * 1024)
            .await,
        Err(StoreError::PrefixTooLarge)
    ));
    let guard = log
        .prepare_prune_candidates("session", Some("current"), 0, 0, None)
        .await
        .unwrap();
    assert!(guard.candidates.is_empty());
    let first = log
        .prepare_prune_candidates("session", Some("current"), 4, 1024 * 1024, None)
        .await
        .unwrap();
    assert_eq!(first.candidates.len(), 4);
    assert!(first.next.is_some());
    end(&log, "current").await;
    open(&log, "pruner", false).await;
    let mut archived = 0;
    let mut cursor = None;
    loop {
        let batch = log
            .prepare_prune_candidates("session", Some("pruner"), 4, 1024 * 1024, cursor.as_ref())
            .await
            .unwrap();
        if batch.candidates.is_empty() {
            break;
        }
        for candidate in &batch.candidates {
            let Some(placeholder) = ArchivedPlaceholder::prepare(
                candidate.event_id.clone(),
                candidate.tool_call_id.clone(),
                candidate.tool_name.clone(),
                &candidate.projection,
            )
            .unwrap() else {
                continue;
            };
            log.append(&event("pruner", Fact::ToolResultArchived { placeholder }))
                .await
                .unwrap();
            archived += 1;
        }
        cursor = batch.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(archived, 40);
    let source = log
        .read_model_context("session", Some("pruner"), 10_000, 512 * 1024)
        .await
        .unwrap();
    assert_eq!(
        source
            .tail
            .iter()
            .filter(|e| matches!(e, ContextEvent::Archived(_)))
            .count(),
        40
    );
    log.close().await.unwrap();
}

#[path = "archive/automatic.rs"]
mod automatic;
#[path = "archive/summary.rs"]
mod summary;
