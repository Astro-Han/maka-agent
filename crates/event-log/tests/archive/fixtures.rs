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
pub(super) const SUMMARY: &str = "## Goal\nContinue the work.\n## Progress\nCompleted the first stage.\n## Next Steps\nVerify the remaining behavior.\n## Critical Context\nPreserve the durable execution facts.";
pub(super) fn event(id: &str, fact: Fact) -> EventWrite {
    EventWrite::plain(RuntimeEvent::new(
        Invocation {
            session_id: "session".into(),
            turn_id: id.into(),
            run_id: id.into(),
            invocation_id: id.into(),
        },
        fact,
    ))
    .unwrap()
}
pub(super) async fn open(log: &EventLog, id: &str, compact: bool) {
    log.append(&event(
        id,
        Fact::InvocationOpened {
            configuration: None,
            input: if compact {
                InvocationInput::ContextCompact {
                    request_fingerprint: id.into(),
                }
            } else {
                InvocationInput::Message {
                    source_messages: Vec::new(),
                    content: "question".into(),
                    request_fingerprint: None,
                }
            },
        },
    ))
    .await
    .unwrap();
}

pub(super) async fn end(log: &EventLog, id: &str) {
    log.append(&event(
        id,
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    ))
    .await
    .unwrap();
}
pub(super) fn request(id: &str, step: &str, source: Option<&ModelContextSource>) -> EventWrite {
    event(
        id,
        Fact::ModelRequested {
            step_id: step.into(),
            model_id: "test".into(),
            purpose: if source.is_some() {
                ModelPurpose::Summary
            } else {
                ModelPurpose::Main
            },
            context: None,
            source_scope: source.map_or_else(
                || LogScope::Session {
                    id: "session".into(),
                },
                |s| s.source_evidence.scope.clone(),
            ),
            source_high_water: source.map_or(0, |s| s.source_evidence.high_water),
            source_digest: source.map_or("fixture".into(), |s| s.source_evidence.digest.clone()),
            input_digest: "fixture".into(),
            route_identity: format!("sha256:{}", "a".repeat(64)),
            checkpoint_event_id: source
                .and_then(|s| s.baseline.as_ref().map(|b| b.event_id.clone())),
            effective_source_digest: source.map(|s| s.effective_source_digest.clone()),
        },
    )
}
pub(super) async fn tool(log: &EventLog, id: &str, step: &str, text: String) -> EventWrite {
    log.append(&request(id, step, None)).await.unwrap();
    let output:ModelStep=serde_json::from_value(json!({"parts":[{"kind":"tool_call","call":{"id":step,"name":"Read","input":{"path":"file"},"provider_executed":false}}],"finish_reason":"tool-calls","usage":{}})).unwrap();
    log.append(&event(
        id,
        Fact::ModelCompleted {
            step_id: step.into(),
            output,
        },
    ))
    .await
    .unwrap();
    let operation = format!("{step}:{step}");
    log.append(&event(
        id,
        Fact::ToolDispatched {
            operation_id: operation.clone(),
            call: ToolCallIdentity::provider(step.into(), step.into()),
            name: "Read".into(),
            input: json!({"path":"file"}),
        },
    ))
    .await
    .unwrap();
    let identity = event(
        id,
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    )
    .event()
    .invocation
    .clone();
    let (write, _) = EventWrite::tool_success(
        format!("result-{step}"),
        std::time::SystemTime::now(),
        identity,
        operation,
        ToolSuccess::from(json!(text)),
    )
    .unwrap();
    log.append(&write).await.unwrap();
    write
}
pub(super) fn archive(writer: &str, target: &EventWrite) -> EventWrite {
    let Fact::ToolSettled {
        operation_id,
        outcome,
    } = &target.event().fact
    else {
        panic!()
    };
    let projection = outcome_projection(outcome);
    event(
        writer,
        Fact::ToolResultArchived {
            placeholder: ArchivedPlaceholder::prepare(
                target.event().id.clone(),
                operation_id.split(':').next().unwrap().into(),
                "Read".into(),
                &projection,
            )
            .unwrap()
            .unwrap(),
        },
    )
}
pub(super) async fn summary_pair(
    log: &EventLog,
    id: &str,
    source: &ModelContextSource,
) -> Vec<EventWrite> {
    let summary_step = format!("{id}-summary");
    log.append(&request(id, &summary_step, Some(source)))
        .await
        .unwrap();
    let output:ModelStep=serde_json::from_value(json!({"parts":[{"kind":"text","text_kind":"text","text":SUMMARY}],"finish_reason":"stop","usage":{}})).unwrap();
    log.append(&event(
        id,
        Fact::ModelCompleted {
            step_id: summary_step.clone(),
            output: output.clone(),
        },
    ))
    .await
    .unwrap();
    let checkpoint = event(
        id,
        Fact::ContextCheckpointRecorded {
            checkpoint: ContextCheckpoint {
                mode: CheckpointMode::Standalone,
                covered_through: source.source_evidence.high_water,
                source_digest: source.source_evidence.digest.clone(),
                previous_checkpoint_id: source.baseline.as_ref().map(|b| b.event_id.clone()),
                summary: TextSummary::from_model_step(&output, false).unwrap(),
                summary_step_id: summary_step,
            },
        },
    );
    let terminal = event(
        id,
        Fact::InvocationEnded {
            outcome: InvocationOutcome::ContextCompactFinished {
                outcome: CompactOutcome::Compacted {
                    checkpoint_id: checkpoint.event().id.clone(),
                },
            },
        },
    );
    vec![checkpoint, terminal]
}
