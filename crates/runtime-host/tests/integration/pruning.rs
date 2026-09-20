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

use super::support::client_probe::ClientFixture;
use maka_runtime::{archive::ArchiveIdentity, context::ModelPurpose, event::Fact};
use serde_json::Value;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_reads_frozen_archives_after_compaction_and_restart_without_repeating_tools()
 {
    let fixture = ClientFixture::new("maka-pruning-");
    let expected_stdout = format!("pruning needle {}\n", "x".repeat(80)).repeat(128);
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--pruning-workspace",
                reopened,
                if reopened {
                    "pruning-reopened"
                } else {
                    "pruning-passed"
                },
            )
            .await;
        let saved: Value =
            serde_json::from_slice(&std::fs::read(fixture.workspace.join("pruning.json")).unwrap())
                .unwrap();
        let event_id = ArchiveIdentity::parse_short_ref(saved["ref"].as_str().unwrap()).unwrap();
        let log = fixture.log().await;
        let (identity, resolved) = log
            .read_archive_by_event("pruning", &event_id)
            .await
            .unwrap()
            .unwrap();
        let raw: Value = serde_json::from_slice(&resolved).unwrap();
        assert_eq!(raw["kind"], "terminal");
        assert_eq!(raw["exitCode"], 0);
        assert_eq!(raw["output"]["stdout"], expected_stdout);
        assert_eq!(raw["output"]["stdoutTruncated"], false);
        assert_eq!(
            log.resolve_tool_result("pruning", &identity.runtime_event_id)
                .await
                .unwrap(),
            maka_runtime::tool_output::ToolOutput::Json(raw)
        );
        assert_eq!(
            log.read_archive("pruning", &identity).await.unwrap(),
            Some(resolved)
        );
        assert_eq!(
            log.read_archive("pruning-other", &identity).await.unwrap(),
            None
        );
        let prefix = log.prefix(500, 4 * 1024 * 1024).await.unwrap();
        let archives: Vec<_> = prefix
            .events
            .iter()
            .filter_map(|stored| match &stored.event.fact {
                Fact::ToolResultArchived { placeholder } => Some((stored, placeholder)),
                _ => None,
            })
            .collect();
        assert_eq!(
            archives.len(),
            1,
            "bounded resource pages must not archive themselves"
        );
        let (archive, placeholder) = archives[0];
        assert_eq!(placeholder.identity, identity);
        let target = prefix
            .events
            .iter()
            .find(|stored| stored.event.id == identity.runtime_event_id)
            .unwrap();
        assert!(matches!(target.event.fact, Fact::ToolSettled { .. }));
        assert!(target.sequence < archive.sequence);
        assert_ne!(target.event.id, archive.event.id);
        assert_eq!(
            prefix
                .events
                .iter()
                .filter(|stored| matches!(
                    &stored.event.fact, Fact::ToolDispatched { name, .. } if name == "Bash"
                ))
                .count(),
            1,
            "archive readback must not repeat the source command"
        );
        let checkpoints: Vec<_> = prefix
            .events
            .iter()
            .filter_map(|stored| match &stored.event.fact {
                Fact::ContextCheckpointRecorded { checkpoint } => Some((stored, checkpoint)),
                _ => None,
            })
            .collect();
        assert_eq!(checkpoints.len(), 1);
        let (recorded, checkpoint) = checkpoints[0];
        assert!(checkpoint.covered_through >= archive.sequence);
        let summaries: Vec<_> = prefix
            .events
            .iter()
            .filter_map(|stored| match &stored.event.fact {
                Fact::ModelRequested {
                    purpose: ModelPurpose::Summary,
                    step_id,
                    effective_source_digest,
                    ..
                } => Some((step_id, effective_source_digest)),
                _ => None,
            })
            .collect();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].0, &checkpoint.summary_step_id);
        assert!(
            summaries[0].1.is_some(),
            "summary binds the effective archived source"
        );
        let source = log
            .read_model_context("pruning", None, 500, 4 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(source.baseline.unwrap().event_id, recorded.event.id);
        let facts = serde_json::to_value(&prefix.events).unwrap();
        if let Some(original) = &original {
            let original: &Vec<Value> = original;
            assert_eq!(&facts.as_array().unwrap()[..original.len()], original);
        } else {
            original = Some(facts.as_array().unwrap().clone());
        }
        log.close().await.unwrap();
    }
}
