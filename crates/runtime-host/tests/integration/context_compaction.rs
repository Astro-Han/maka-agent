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
use maka_runtime::{
    context::CompactOutcome,
    event::{Fact, InvocationInput, InvocationOutcome},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_compacts_only_model_history_and_reopens_the_durable_baseline() {
    let fixture = ClientFixture::new("maka-context-");
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--context-compact-workspace",
                reopened,
                if reopened {
                    "context-compact-reopened"
                } else {
                    "context-compact-passed"
                },
            )
            .await;
        let log = fixture.log().await;
        let prefix = log.prefix(500, 4 * 1024 * 1024).await.unwrap();
        let compact_invocations: Vec<_> = prefix
            .events
            .iter()
            .filter(|stored| {
                matches!(
                    stored.event.fact,
                    Fact::InvocationOpened {
                        input: InvocationInput::ContextCompact { .. },
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(compact_invocations.len(), 2);
        let checkpoints: Vec<_> = prefix
            .events
            .iter()
            .filter(|stored| matches!(stored.event.fact, Fact::ContextCheckpointRecorded { .. }))
            .collect();
        assert_eq!(
            checkpoints.len(),
            1,
            "a rejected repair must retain the old baseline"
        );
        let checkpoint = checkpoints[0];
        let source = log
            .read_model_context("context-compact", None, 500, 4 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(source.baseline.unwrap().event_id, checkpoint.event.id);
        let outcomes: Vec<_> = prefix
            .events
            .iter()
            .filter_map(|stored| match &stored.event.fact {
                Fact::InvocationEnded {
                    outcome: InvocationOutcome::ContextCompactFinished { outcome },
                } => Some(outcome),
                _ => None,
            })
            .collect();
        assert!(
            matches!(outcomes.as_slice(), [CompactOutcome::Compacted { checkpoint_id }, CompactOutcome::Failed { .. }] if checkpoint_id == &checkpoint.event.id)
        );
        let last_request = prefix
            .events
            .iter()
            .rev()
            .find_map(|stored| match &stored.event.fact {
                Fact::ModelRequested {
                    checkpoint_event_id,
                    ..
                } => Some(checkpoint_event_id),
                _ => None,
            })
            .unwrap();
        assert_eq!(last_request.as_deref(), Some(checkpoint.event.id.as_str()));
        let facts = serde_json::to_value(&prefix.events).unwrap();
        if let Some(original) = &original {
            let original: &Vec<serde_json::Value> = original;
            assert_eq!(&facts.as_array().unwrap()[..original.len()], original);
        } else {
            original = Some(facts.as_array().unwrap().clone());
        }
    }
}
