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
use maka_runtime::archive::projection_digest;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn stale_after_opening_binds_summary_and_late_prune_rolls_back_checkpoint() {
    for late in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("summary.sqlite");
        let log = EventLog::open(&path).await.unwrap();
        open(&log, "old", false).await;
        let first = tool(&log, "old", "first", "a".repeat(12_000)).await;
        let second = tool(&log, "old", "second", "b".repeat(12_000)).await;
        end(&log, "old").await;
        open(&log, "compact", true).await;
        let stale = log
            .prepare_context_compaction(
                "session",
                Some("compact"),
                100,
                64 * 1024,
                &CheckpointMode::Standalone,
            )
            .await
            .unwrap();
        let archived = archive("compact", &first);
        log.append(&archived).await.unwrap();
        assert!(
            log.append(&request("compact", "stale-summary", Some(&stale)))
                .await
                .is_err()
        );
        let fresh = log
            .prepare_context_compaction(
                "session",
                Some("compact"),
                100,
                64 * 1024,
                &CheckpointMode::Standalone,
            )
            .await
            .unwrap();
        assert_eq!(fresh.source_evidence.digest, stale.source_evidence.digest);
        assert_ne!(fresh.effective_source_digest, stale.effective_source_digest);
        let Fact::ToolResultArchived { placeholder } = &archived.event().fact else {
            panic!()
        };
        let projected = fresh
            .tail
            .iter()
            .find_map(|entry| match entry {
                ContextEvent::Archived(result) if result.event_id == first.event().id => {
                    Some(result)
                }
                _ => None,
            })
            .unwrap();
        let replacement_digest = projection_digest(&projected.replacement).unwrap();
        let evidence = serde_json::to_vec(&(
            projected.sequence,
            &projected.event_id,
            placeholder,
            replacement_digest,
        ))
        .unwrap();
        let mut hash = Sha256::new();
        hash.update(b"maka.effective-context.v1\0");
        hash.update(fresh.source_evidence.digest.as_bytes());
        hash.update((evidence.len() as u64).to_le_bytes());
        hash.update(evidence);
        assert_eq!(
            fresh.effective_source_digest,
            format!("sha256:{:x}", hash.finalize()),
            "summary proof must bind the actual model replacement plus full archive evidence"
        );
        let pair = summary_pair(&log, "compact", &fresh).await;
        if late {
            log.append(&archive("compact", &second)).await.unwrap();
            assert!(log.append_batch(&pair).await.is_err());
            assert!(
                log.read_model_context("session", Some("compact"), 100, 64 * 1024)
                    .await
                    .unwrap()
                    .baseline
                    .is_none()
            );
            assert!(
                log.append(&request("compact", "repair", Some(&fresh)))
                    .await
                    .is_err()
            );
        } else {
            log.append_batch(&pair).await.unwrap();
            let Fact::ToolResultArchived { placeholder } = &archived.event().fact else {
                panic!()
            };
            let expected = log
                .read_archive("session", &placeholder.identity)
                .await
                .unwrap()
                .unwrap();
            open(&log, "next-compact", true).await;
            assert!(log.append(&archive("next-compact", &second)).await.is_err());
            let source = log
                .prepare_context_compaction(
                    "session",
                    Some("next-compact"),
                    100,
                    64 * 1024,
                    &CheckpointMode::Standalone,
                )
                .await
                .unwrap();
            let pair = summary_pair(&log, "next-compact", &source).await;
            log.append_batch(&pair).await.unwrap();
            log.close().await.unwrap();
            let log = EventLog::open(&path).await.unwrap();
            log.append(&archived).await.unwrap();
            assert_eq!(
                log.read_archive("session", &placeholder.identity)
                    .await
                    .unwrap()
                    .unwrap(),
                expected
            );
            log.read_model_context("session", None, 100, 64 * 1024)
                .await
                .unwrap();
            let imported = archive("next-compact", &second);
            let inspect = rusqlite::Connection::open(&path).unwrap();
            inspect.execute("INSERT INTO event_log(event_id,invocation_id,kind,operation_id,event_json) VALUES (?,'next-compact','tool_result_archived',NULL,?)",
                rusqlite::params![imported.event().id,serde_json::to_string(imported.event()).unwrap()]).unwrap();
            assert!(
                log.read_model_context("session", None, 100, 64 * 1024)
                    .await
                    .is_err(),
                "read must reject imported post-summary prune, not reinterpret the old summary"
            );
            log.close().await.unwrap();
            continue;
        }
        log.close().await.unwrap();
    }
}

#[tokio::test]
async fn prune_scan_advances_past_small_results_and_freezes_target_fence() {
    let directory = tempfile::tempdir().unwrap();
    let log = EventLog::open(&directory.path().join("scan.sqlite"))
        .await
        .unwrap();
    open(&log, "active", false).await;
    for n in 0..129 {
        tool(&log, "active", &format!("small-{n}"), "small".into()).await;
    }
    let large = tool(&log, "active", "large", "x".repeat(12_000)).await;
    let first = log
        .prepare_prune_candidates("session", Some("active"), 128, 1024 * 1024, None)
        .await
        .unwrap();
    assert_eq!(first.candidates.len(), 128);
    let cursor = first.next.unwrap();
    let newer = tool(&log, "active", "after-fence", "y".repeat(12_000)).await;
    let next = log
        .prepare_prune_candidates("session", Some("active"), 128, 1024 * 1024, Some(&cursor))
        .await
        .unwrap();
    assert!(
        next.candidates
            .iter()
            .any(|c| c.event_id == large.event().id)
    );
    assert!(
        !next
            .candidates
            .iter()
            .any(|c| c.event_id == newer.event().id)
    );
    assert!(next.next.is_none());
    log.close().await.unwrap();
}
