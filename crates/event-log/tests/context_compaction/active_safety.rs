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
async fn partial_and_unsettled_provider_steps_are_not_active_cut_boundaries() {
    for provider in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let log = EventLog::open(&directory.path().join("provider.sqlite"))
            .await
            .unwrap();
        let anchor = opening("active");
        log.append(&anchor).await.unwrap();
        let source = log
            .read_model_context("session", Some("active"), 100, 8192)
            .await
            .unwrap();
        log.append(&request("active", "main", ModelPurpose::Main, &source))
            .await
            .unwrap();
        if provider {
            log.append(&event("active", Fact::ModelCompleted { step_id: "main".into(),
                output: serde_json::from_value(json!({
                    "parts":[{"kind":"tool_call","call":{"id":"remote","name":"search","input":{},"provider_executed":true}}],
                    "finish_reason":"stop","usage":{}
                })).unwrap()
            })).await.unwrap();
        } else {
            log.append(&event(
                "active",
                Fact::ModelObserved {
                    step_id: "main".into(),
                    event: ModelEvent::PartDelta {
                        id: "partial".into(),
                        text: "visible".into(),
                        provider_options: None,
                    },
                },
            ))
            .await
            .unwrap();
            log.append(&event(
                "active",
                Fact::ModelInterrupted {
                    step_id: "main".into(),
                    status: maka_runtime::event::ModelInterruption::Failed,
                },
            ))
            .await
            .unwrap();
        }
        let mode = CheckpointMode::MidTurn {
            anchor_event_id: anchor.event().id.clone(),
        };
        assert!(
            log.prepare_context_compaction("session", Some("active"), 100, 8192, &mode)
                .await
                .is_err()
        );
        log.close().await.unwrap();
    }
}
