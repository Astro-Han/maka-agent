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

use maka_event_log::{EventLog, StoreError, context::ModelContextSource};
use maka_runtime::{
    context::{CompactOutcome, ContextCheckpoint, TextSummary},
    event::{
        EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, LogScope, RuntimeEvent,
    },
    model::{ModelEvent, ModelStep},
    tool_call::ToolCallIdentity,
};
use serde_json::json;

#[path = "context_compaction/active.rs"]
mod active;
#[path = "context_compaction/fixtures.rs"]
mod fixtures;
#[path = "context_compaction/frozen.rs"]
mod frozen;
use fixtures::{SUMMARY, closed, prepare_trace, prepared};

#[tokio::test]
async fn repeated_checkpoints_cross_old_history_limits_with_bounded_tail_and_exact_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let mut last = Vec::new();
    for round in 0..3 {
        closed(&log, &format!("main-{round}"), 3 * 1024 * 1024, 3400).await;
        if round == 0 {
            let source = log
                .read_model_context("session", None, 10_000, 8 * 1024 * 1024)
                .await
                .unwrap();
            let prefix = log
                .scoped_prefix(
                    LogScope::Session {
                        id: "session".into(),
                    },
                    10_000,
                    8 * 1024 * 1024,
                )
                .await
                .unwrap();
            assert_eq!(source.source_evidence.digest, prefix.digest);
        }
        last = prepare_trace(
            &log,
            &format!("compact-{round}"),
            if round == 2 { 10_001 } else { 0 },
            false,
        )
        .await;
        let commits = log.subscribe_commits();
        assert!(
            log.append(&last[0]).await.is_err(),
            "checkpoint without atomic domain terminal must roll back"
        );
        assert!(!commits.has_changed().unwrap());
        log.append_batch(&last).await.unwrap();
    }
    prepare_trace(&log, "failed-compact", 10_001, true).await;
    assert!(matches!(
        log.scoped_prefix(
            LogScope::Session {
                id: "session".into()
            },
            10_000,
            8 * 1024 * 1024
        )
        .await,
        Err(StoreError::PrefixTooLarge)
    ));
    let source = log
        .read_model_context("session", None, 100, 8192)
        .await
        .unwrap();
    assert_eq!(source.baseline.unwrap().event_id, last[0].event().id);
    assert!(source.tail.len() < 10);
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    let commits = log.subscribe_commits();
    log.append_batch(&last).await.unwrap();
    assert!(!commits.has_changed().unwrap());
    assert_eq!(
        log.read_model_context("session", None, 100, 8192)
            .await
            .unwrap()
            .source_evidence
            .digest,
        source.source_evidence.digest
    );
    assert!(
        log.read_model_context("other", None, 100, 8192)
            .await
            .unwrap()
            .baseline
            .is_none()
    );
    let oversized = fixtures::event(
        "oversized",
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                source_messages: Vec::new(),
                content: "x".repeat(9 * 1024 * 1024).into(),
                request_fingerprint: None,
            },
        },
    );
    log.append(&oversized).await.unwrap();
    log.invocation_recovery(&oversized.event().invocation, 100, 8192)
        .await
        .unwrap();
    log.append(&fixtures::event(
        "oversized",
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Cancelled {
                source: "recovery".into(),
            },
        },
    ))
    .await
    .unwrap();
    assert!(log.unfinished_invocations(10).await.unwrap().is_empty());
    log.close().await.unwrap();
}

#[tokio::test]
async fn forged_summary_source_and_hidden_old_unknowns_never_become_a_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let observed = closed(&log, "main", 32, 1).await;
    let pair = prepared(&log, "compact").await;
    for mutation in [0, 1, 2] {
        let mut forged = pair[0].event().clone();
        let Fact::ContextCheckpointRecorded { checkpoint } = &mut forged.fact else {
            panic!()
        };
        match mutation {
            0 => checkpoint.source_digest = format!("sha256:{}", "0".repeat(64)),
            1 => checkpoint.summary.text = SUMMARY.replace("first", "second"),
            _ => checkpoint.previous_checkpoint_id = Some(forged.id.clone()),
        }
        assert!(
            log.append_batch(&[EventWrite::plain(forged).unwrap(), pair[1].clone()])
                .await
                .is_err()
        );
    }
    log.append_batch(&pair).await.unwrap();
    let inspect = rusqlite::Connection::open(&path).unwrap();
    let original: String = inspect
        .query_row(
            "SELECT event_json FROM runtime_events WHERE event_id = ?",
            [&observed],
            |row| row.get(0),
        )
        .unwrap();
    inspect.execute("UPDATE event_log SET event_json = replace(event_json, 'part', 'changed') WHERE event_id = ?", [&observed]).unwrap();
    assert!(
        log.read_model_context("session", None, 100, 8192)
            .await
            .is_err()
    );
    let mut unknown: RuntimeEvent = serde_json::from_str(&original).unwrap();
    unknown.fact = Fact::ToolDispatched {
        operation_id: "hidden".into(),
        call: ToolCallIdentity::standalone("hidden-call".into()),
        name: "write".into(),
        input: json!({}),
    };
    inspect.execute("UPDATE event_log SET kind = 'tool_dispatched', operation_id = 'hidden', event_json = ? WHERE event_id = ?",
        rusqlite::params![serde_json::to_string(&unknown).unwrap(), observed]).unwrap();
    let error = log
        .read_model_context("session", None, 100, 8192)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unresolved prior execution"));
    assert!(
        log.prepare_context_compaction(
            "session",
            None,
            100,
            8192,
            &maka_runtime::context::CheckpointMode::Standalone
        )
        .await
        .is_err()
    );
    log.close().await.unwrap();
}
