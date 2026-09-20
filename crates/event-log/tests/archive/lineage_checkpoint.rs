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

#[tokio::test]
async fn narrower_checkpoint_does_not_corrupt_later_whole_session_archives() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("checkpoint-archive.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let source = continuation::opening("source", None);
    continuation::append(&log, &source).await;
    continuation::close(&log, &source).await;
    let base = log
        .context_before_run(&source.invocation, 100, 65536)
        .await
        .unwrap()
        .source_evidence;
    let claim = continuation::claim(
        &log,
        "claim",
        &source,
        SessionBase {
            high_water: base.high_water,
            digest: base.digest,
        },
    )
    .await;
    open(&log, "unrelated", false).await;
    let target = tool(&log, "unrelated", "unrelated-tool", "body".repeat(3000)).await;
    end(&log, "unrelated").await;
    let child = continuation::opening("child", Some(claim));
    continuation::append(&log, &child).await;
    let active = log
        .read_model_context("session", Some("child"), 100, 65536)
        .await
        .unwrap();
    let mut main = request("child", "main", Some(&active)).event().clone();
    if let Fact::ModelRequested { purpose, .. } = &mut main.fact {
        *purpose = ModelPurpose::Main;
    }
    continuation::append(&log, &main).await;
    log.append(&event("child", Fact::ModelCompleted {
        step_id: "main".into(),
        output: serde_json::from_value(json!({"parts":[{"kind":"text","text_kind":"text","text":"work"}],"finish_reason":"stop","usage":{}})).unwrap(),
    })).await.unwrap();
    let mode = CheckpointMode::MidTurn {
        anchor_event_id: child.id.clone(),
    };
    let compact = log
        .prepare_context_compaction("session", Some("child"), 100, 65536, &mode)
        .await
        .unwrap();
    let pair = summary_pair(&log, "child", &compact).await;
    let mut checkpoint = pair[0].event().clone();
    if let Fact::ContextCheckpointRecorded { checkpoint } = &mut checkpoint.fact {
        checkpoint.mode = mode;
    }
    continuation::append(&log, &checkpoint).await;
    continuation::close(&log, &child).await;
    open(&log, "writer", false).await;
    let archived = archive("writer", &target);
    log.append(&archived).await.unwrap();
    let Fact::ToolResultArchived { placeholder } = &archived.event().fact else {
        panic!()
    };
    let body = log
        .read_archive("session", &placeholder.identity)
        .await
        .unwrap()
        .unwrap();
    let current = log
        .read_model_context("session", Some("writer"), 100, 65536)
        .await
        .unwrap();
    assert!(current.baseline.is_none());
    assert!(
        current
            .tail
            .iter()
            .any(|e| matches!(e, ContextEvent::Archived(e) if e.event_id == target.event().id))
    );
    end(&log, "writer").await;
    // Verify that the existing narrower proof also remains readable after this archive.
    let boundary = log
        .run_prefix("session", "child", None, 100, 65536)
        .await
        .unwrap()
        .unwrap();
    let selected = log
        .read_lineage_context(
            &RunBoundary {
                invocation: boundary.invocation,
                high_water: boundary.high_water,
                digest: boundary.digest,
            },
            100,
            65536,
        )
        .await
        .unwrap();
    assert_eq!(selected.baseline.unwrap().event_id, checkpoint.id);
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.read_archive("session", &placeholder.identity)
            .await
            .unwrap()
            .unwrap(),
        body
    );
    log.append(&archived).await.unwrap();
    log.close().await.unwrap();
}
