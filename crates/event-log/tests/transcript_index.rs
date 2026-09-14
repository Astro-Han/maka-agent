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
use maka_runtime::{
    event::{
        Fact, Invocation, InvocationInput, InvocationOutcome, LogScope, ModelInterruption,
        RuntimeEvent,
    },
    model::{ModelEvent, ModelPart, ModelStep, ModelUsage, TextKind},
};
use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[path = "transcript_index/fixtures.rs"]
mod fixtures;
use fixtures::*;

#[tokio::test]
async fn completed_bytes_digest_identity_reopen_and_exact_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("log.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    assert!(log.prepare_transcript("a", 0, 32).await.unwrap());
    assert!(
        log.transcript_headers(
            "a",
            &maka_event_log::transcript::TranscriptRead {
                through: maka_presentation::watermark(0).unwrap(),
                position: 0,
                direction: maka_event_log::transcript::TranscriptDirection::Newer,
                limit: 1,
            }
        )
        .await
        .unwrap()
        .is_empty()
    );
    let (id, _) = partial(&log, "a", "hello😀").await;
    log.append(&event(
        "a",
        observe(
            "a",
            ModelEvent::PartFinished {
                id: "provider-part".into(),
                provider_options: None,
            },
        ),
    ))
    .await
    .unwrap();
    log.append(&event(
        "a",
        Fact::ModelCompleted {
            step_id: "step-a".into(),
            output: ModelStep {
                parts: vec![ModelPart::Text {
                    text_kind: TextKind::Text,
                    text: "hello😀".into(),
                    provider_options: None,
                }],
                finish_reason: maka_runtime::model::ModelFinishReason::Stop,
                usage: ModelUsage {
                    input_tokens: Some(9),
                    output_tokens: Some(3),
                    ..Default::default()
                },
                provider_options: None,
                response_id: None,
                model: None,
                timestamp: None,
            },
        },
    ))
    .await
    .unwrap();
    let through = end(&log, "a", InvocationOutcome::Completed).await;
    let wake = log.subscribe_commits();
    assert!(log.prepare_transcript("a", through, 32).await.unwrap());
    assert!(!wake.has_changed().unwrap());
    let db = Connection::open(&path).unwrap();
    let original = rows(&db, "a");
    assert_eq!(original.len(), 4);
    let assistant: Value = serde_json::from_slice(&original[1].1).unwrap();
    assert_eq!(assistant["id"], id);
    assert_eq!(assistant["modelId"], "real-model");
    for (_, bytes, digest, size) in &original {
        assert_eq!(*size, bytes.len() as i64);
        assert_eq!(*digest, format!("sha256:{:x}", Sha256::digest(bytes)));
    }
    // Derived progress may be discarded; replay must verify exactly the same rows.
    db.execute("DELETE FROM transcript_progress", []).unwrap();
    assert!(log.prepare_transcript("a", through, 32).await.unwrap());
    assert_eq!(rows(&db, "a"), original);
    log.close().await.unwrap();
    let reopened = EventLog::open(&path).await.unwrap();
    assert!(reopened.prepare_transcript("a", through, 32).await.unwrap());
    assert_eq!(rows(&db, "a"), original);
    // Corruption cannot silently replace an already published payload.
    db.execute(
        "UPDATE transcript_rows SET payload = X'00' WHERE sequence = ?",
        [original[1].0],
    )
    .unwrap();
    db.execute("DELETE FROM transcript_progress", []).unwrap();
    assert!(matches!(
        reopened.prepare_transcript("a", through, 32).await,
        Err(StoreError::TranscriptConflict)
    ));
    assert_eq!(progress(&db, "a"), 0);
}

#[tokio::test]
async fn partial_freezes_at_interruption_and_old_fence_excludes_later_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("log.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let (id, live) = partial(&log, "a", "partial😀").await;
    let interrupted = interrupt(&log, "a").await;
    let ended = end(
        &log,
        "a",
        InvocationOutcome::Cancelled {
            source: "user".into(),
        },
    )
    .await;
    let db = Connection::open(&path).unwrap();
    let invocation = event("a", opening()).event().invocation.clone();
    let overlay = log.active_transcript(&invocation, live).await.unwrap();
    assert_eq!(overlay.len(), 1);
    assert_eq!(overlay[0].id, id);
    assert!(
        log.active_transcript(&invocation, interrupted)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        log.active_transcript(&invocation, ended)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(log.prepare_transcript("a", live, 32).await.unwrap());
    assert_eq!(rows(&db, "a").len(), 1);
    assert_eq!(progress(&db, "a"), live as i64);
    assert!(log.prepare_transcript("a", interrupted, 32).await.unwrap());
    let frozen = rows(&db, "a");
    assert_eq!(frozen.len(), 2);
    let assistant: Value = serde_json::from_slice(&frozen[1].1).unwrap();
    assert_eq!(assistant["id"], id);
    assert_eq!(assistant["text"], "partial😀");
    assert!(log.prepare_transcript("a", ended, 32).await.unwrap());
    let final_rows = rows(&db, "a");
    assert_eq!(&final_rows[..2], frozen.as_slice());
    assert_eq!(final_rows.len(), 3); // no invented unknown usage
    let terminal: Value = serde_json::from_slice(&final_rows[2].1).unwrap();
    assert_eq!(terminal["status"], "aborted");
}

#[tokio::test]
async fn bounded_progress_and_session_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("log.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.append(&event("a", opening())).await.unwrap();
    log.append(&event("b", opening())).await.unwrap();
    let a_end = end(&log, "a", InvocationOutcome::Completed).await;
    let through = end(&log, "b", InvocationOutcome::Completed).await;
    let db = Connection::open(&path).unwrap();
    assert!(!log.prepare_transcript("a", through, 1).await.unwrap());
    assert_eq!(progress(&db, "a"), 1);
    assert!(log.prepare_transcript("a", through, 1).await.unwrap());
    assert_eq!(progress(&db, "a"), through as i64);
    assert_eq!(rows(&db, "a").last().unwrap().0, (a_end * 256) as i64);
    assert!(rows(&db, "b").is_empty());
    assert!(log.prepare_transcript("a", through + 1, 1).await.is_err());
    assert!(log.prepare_transcript("a", through, 33).await.is_err());
}

#[tokio::test]
async fn terminal_without_model_interruption_seals_only_committed_partial_evidence() {
    for outcome in [
        InvocationOutcome::Failed {
            class: "event_commit".into(),
            message: None,
        },
        InvocationOutcome::Cancelled {
            source: "shutdown".into(),
        },
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("log.sqlite");
        let log = EventLog::open(&path).await.unwrap();
        let (id, live) = partial(&log, "a", "committed😀 prefix").await;
        let invocation = event("a", opening()).event().invocation.clone();
        let overlay = log.active_transcript(&invocation, live).await.unwrap();
        assert!(log.prepare_transcript("a", live, 32).await.unwrap());
        // A persistence fault may prevent ModelInterrupted while a later
        // terminal append succeeds. The source facts allow this boundary.
        let through = end(&log, "a", outcome.clone()).await;
        assert!(log.prepare_transcript("a", through, 32).await.unwrap());
        assert!(
            log.active_transcript(&invocation, through)
                .await
                .unwrap()
                .is_empty()
        );
        let db = Connection::open(&path).unwrap();
        let original = rows(&db, "a");
        assert_eq!(original.len(), 3); // user, factual partial, terminal; no usage
        let assistant: Value = serde_json::from_slice(&original[1].1).unwrap();
        assert_eq!(assistant["id"], id);
        let failed = matches!(outcome, InvocationOutcome::Failed { .. });
        let mut expected = serde_json::to_value(&overlay[0]).unwrap();
        if failed {
            expected["interrupted"] = Value::Bool(true);
        }
        assert_eq!(assistant, expected);
        let delivery = log
            .session_stream_events("a", live, through, 8, 4096)
            .await
            .unwrap();
        assert!(matches!(&delivery.events[0].fact,
            maka_event_log::observation::StreamFact::InvocationEnded { interrupted, .. }
            if if failed { interrupted == std::slice::from_ref(&id) } else { interrupted.is_empty() }));
        assert_eq!(original[1].0, (through * 256) as i64);
        let terminal: Value = serde_json::from_slice(&original[2].1).unwrap();
        assert_eq!(
            terminal["status"],
            if matches!(outcome, InvocationOutcome::Failed { .. }) {
                "failed"
            } else {
                "aborted"
            }
        );
        // Index reset and process reopen must not drop or relabel the prefix.
        db.execute("DELETE FROM transcript_progress", []).unwrap();
        log.close().await.unwrap();
        let reopened = EventLog::open(&path).await.unwrap();
        assert!(reopened.prepare_transcript("a", through, 32).await.unwrap());
        assert_eq!(rows(&db, "a"), original);
        assert_eq!(
            reopened.active_transcript(&invocation, live).await.unwrap(),
            overlay
        );
    }
}

#[tokio::test]
async fn oversized_evidence_and_tool_content_fail_without_committing_progress() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("log.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    partial(&log, "a", &"x".repeat(16 * 1024 * 1024)).await;
    let through = interrupt(&log, "a").await;
    let db = Connection::open(&path).unwrap();
    assert!(matches!(
        log.prepare_transcript("a", through, 32).await,
        Err(StoreError::PrefixTooLarge)
    ));
    assert_eq!(progress(&db, "a"), 0);
    assert!(rows(&db, "a").is_empty());
    log.append(&event("b", opening())).await.unwrap();
    let through = log
        .append(&event(
            "b",
            Fact::ToolDispatched {
                operation_id: "tool".into(),
                call: maka_runtime::tool_call::ToolCallIdentity::standalone("tool-call".into()),
                name: "file".into(),
                input: Value::Null,
            },
        ))
        .await
        .unwrap();
    assert!(matches!(
        log.prepare_transcript("b", through, 32).await,
        Err(StoreError::Projection(_))
    ));
    assert_eq!(progress(&db, "b"), 0);
    assert!(rows(&db, "b").is_empty());
}
