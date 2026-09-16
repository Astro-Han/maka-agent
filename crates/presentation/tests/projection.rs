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

use maka_presentation::{Content, InvocationView, ProjectionError, watermark};
use maka_runtime::event::{
    Fact, Invocation, InvocationInput, InvocationOutcome, LogScope, MessageInput,
    ModelInterruption, RuntimeEvent, StoredEvent,
};
use maka_runtime::model::{ModelEvent, ModelPart, ModelStep, ModelUsage, TextKind};
use serde_json::json;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, UNIX_EPOCH};

fn events(outcome: InvocationOutcome) -> Vec<StoredEvent> {
    let invocation = Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    let options = Some(json!({"vendor":{"signature":"signed"}}));
    let usage = ModelUsage {
        input_tokens: Some(2),
        output_tokens: Some(3),
        ..Default::default()
    };
    let mut facts = vec![
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                skill_invocation: Default::default(),
                source_messages: Vec::new(),
                content: MessageInput {
                    display_text: Some("visible input".into()),
                    .."model input".into()
                },
                request_fingerprint: Some("admission".into()),
            },
        },
        Fact::ModelRequested {
            effective_source_digest: None,
            purpose: None,
            context: None,
            checkpoint_event_id: None,
            step_id: "step".into(),
            model_id: "historical-model".into(),
            source_scope: LogScope::Session {
                id: "session".into(),
            },
            source_high_water: 1,
            source_digest: "source".into(),
            input_digest: "input".into(),
            route_identity: "route".into(),
        },
        Fact::ModelObserved {
            step_id: "step".into(),
            event: ModelEvent::PartStarted {
                id: "provider-part".into(),
                text_kind: TextKind::Text,
                provider_options: None,
            },
        },
        Fact::ModelObserved {
            step_id: "step".into(),
            event: ModelEvent::PartDelta {
                id: "provider-part".into(),
                text: "😀 hello".into(),
                provider_options: options.clone(),
            },
        },
    ];
    if matches!(
        outcome,
        InvocationOutcome::Completed | InvocationOutcome::HandoffPaused { .. }
    ) {
        facts.extend([
            Fact::ModelObserved {
                step_id: "step".into(),
                event: ModelEvent::PartFinished {
                    id: "provider-part".into(),
                    provider_options: None,
                },
            },
            Fact::ModelObserved {
                step_id: "step".into(),
                event: ModelEvent::Finished {
                    reason: maka_runtime::model::ModelFinishReason::Stop,
                    usage: usage.clone(),
                    provider_options: None,
                },
            },
            Fact::ModelCompleted {
                step_id: "step".into(),
                output: ModelStep {
                    parts: vec![ModelPart::Text {
                        text_kind: TextKind::Text,
                        text: "😀 hello".into(),
                        provider_options: options,
                    }],
                    finish_reason: maka_runtime::model::ModelFinishReason::Stop,
                    usage,
                    provider_options: None,
                    response_id: None,
                    model: None,
                    timestamp: None,
                },
            },
        ]);
    } else {
        facts.push(Fact::ModelInterrupted {
            step_id: "step".into(),
            status: if matches!(outcome, InvocationOutcome::Cancelled { .. }) {
                ModelInterruption::Cancelled
            } else {
                ModelInterruption::Failed
            },
        });
    }
    facts.push(Fact::InvocationEnded { outcome });
    facts
        .into_iter()
        .enumerate()
        .map(|(index, fact)| {
            let mut event = RuntimeEvent::new(invocation.clone(), fact);
            event.recorded_at = UNIX_EPOCH + Duration::from_millis(1_000 + index as u64);
            StoredEvent {
                sequence: 10 + index as u64 * 2,
                event,
            }
        })
        .collect()
}

#[test]
fn completed_and_interrupted_rows_keep_overlay_identity_time_and_original_decoder_contract() {
    let mut messages = Vec::new();
    for outcome in [
        InvocationOutcome::Completed,
        InvocationOutcome::Cancelled {
            source: "user".into(),
        },
        InvocationOutcome::Failed {
            class: "provider".into(),
            message: Some("factual failure".into()),
        },
        InvocationOutcome::HandoffPaused {
            pause: maka_runtime::handoff::HandoffPause {
                intent: maka_runtime::handoff::HandoffIntent {
                    handoff_id: "handoff".into(),
                    host_epoch: "host".into(),
                    root_run_id: "run".into(),
                    successor_run_id: "successor-run".into(),
                    successor_invocation_id: "successor-invocation".into(),
                    claim_id: "claim".into(),
                },
                remaining_steps: std::num::NonZeroU16::new(2).unwrap(),
                execution: Box::new(maka_runtime::handoff::HandoffExecution {
                    replay: maka_runtime::continuation::ReplayEvidence {
                        version: maka_runtime::continuation::REPLAY_VERSION,
                        digest: format!("sha256:{}", "a".repeat(64)),
                        route_identity: format!("sha256:{}", "b".repeat(64)),
                    },
                    context: None,
                    provider_options: serde_json::json!({}),
                    main_output_limit: None,
                    supports_vision: false,
                    tools: maka_runtime::handoff::HandoffTools {
                        catalog_digest: format!("sha256:{}", "c".repeat(64)),
                        loaded: Default::default(),
                    },
                    compaction: maka_runtime::handoff::CompactionBudget::Available,
                    replay_base: None,
                }),
            },
        },
    ] {
        let events = events(outcome.clone());
        let mut view = InvocationView::new(1024).unwrap();
        let mut replay = InvocationView::new(1024).unwrap();
        let mut rows = Vec::new();
        let mut frozen = Vec::new();
        for (index, stored) in events.iter().enumerate() {
            let next = view.push(stored).unwrap();
            let roundtrip = StoredEvent {
                sequence: stored.sequence,
                event: serde_json::from_slice(&serde_json::to_vec(&stored.event).unwrap()).unwrap(),
            };
            assert_eq!(next, replay.push(&roundtrip).unwrap());
            if index == 3 {
                assert!(
                    next.is_empty(),
                    "observation does not publish a mutable durable row"
                );
                frozen = view.overlay();
                assert_eq!(frozen.len(), 1);
                assert_eq!(frozen[0].id, events[2].event.id);
                assert_eq!(frozen[0].ts, 1002);
            }
            rows.extend(next);
        }
        assert!(view.overlay().is_empty());
        let assistant = rows
            .iter()
            .find(|row| matches!(row.message.content, Content::Assistant { .. }))
            .unwrap();
        if let Content::Assistant { interrupted, .. } = &mut frozen[0].content {
            assert!(!*interrupted, "live overlay is not a failed fragment");
            *interrupted = matches!(outcome, InvocationOutcome::Failed { .. });
        }
        assert_eq!(
            assistant.message, frozen[0],
            "overlay and settled presentation keep the same content identity"
        );
        assert_eq!(rows[0].message.ts, 1000);
        assert!(
            matches!(&rows[0].message.content, Content::User { text, display_text, .. }
            if text == "model input" && display_text.as_deref() == Some("visible input"))
        );
        assert!(
            matches!(&assistant.message.content, Content::Assistant { model_id, .. } if model_id == "historical-model")
        );
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(row.message.content, Content::TokenUsage { .. }))
                .count(),
            usize::from(matches!(
                outcome,
                InvocationOutcome::Completed | InvocationOutcome::HandoffPaused { .. }
            )),
            "no fabricated usage after interruption"
        );
        assert!(
            rows.windows(2)
                .all(|rows| rows[0].sequence < rows[1].sequence)
        );
        assert!(
            rows.last().unwrap().sequence <= watermark(events.last().unwrap().sequence).unwrap()
        );
        let terminal = serde_json::to_value(&rows.last().unwrap().message).unwrap();
        let expected = match outcome {
            InvocationOutcome::Completed | InvocationOutcome::ContextCompactFinished { .. } => {
                Some("completed")
            }
            InvocationOutcome::Cancelled { .. } => Some("aborted"),
            InvocationOutcome::Failed { .. } => Some("failed"),
            InvocationOutcome::HandoffPaused { .. } => None,
        };
        if let Some(expected) = expected {
            assert_eq!(terminal["id"], events.last().unwrap().event.id);
            assert_eq!(terminal["status"], expected);
        } else {
            assert!(
                !rows
                    .iter()
                    .any(|row| matches!(row.message.content, Content::TurnState { .. })),
                "physical pause must not publish a logical Turn terminal"
            );
        }
        messages.extend(rows.into_iter().map(|row| row.message));
    }
    // A closed part is not a successful request. Only provider-finalized
    // reasoning keeps its normal presentation after the surrounding request fails.
    for (options, finalized) in [
        (json!({}), false),
        (json!({"anthropic":{"signature":"signed"}}), true),
        (json!({"anthropic":{"redactedData":"opaque"}}), true),
        (
            json!({"openai":{"itemId":"r","reasoningEncryptedContent":"opaque"}}),
            true,
        ),
        (
            json!({"makaResponses":{"version":1,"profile":"relay","itemId":"r","summaryPartLengths":[2]}}),
            true,
        ),
        (
            json!({"makaResponses":{"version":1,"profile":"relay","itemId":"r","summaryPartLengths":[-1]}}),
            false,
        ),
    ] {
        for kind in [TextKind::Text, TextKind::Thinking] {
            let mut facts = events(InvocationOutcome::Failed {
                class: "provider".into(),
                message: None,
            });
            if let Fact::ModelObserved {
                event:
                    ModelEvent::PartStarted {
                        text_kind,
                        provider_options,
                        ..
                    },
                ..
            } = &mut facts[2].event.fact
            {
                *text_kind = kind;
                *provider_options = Some(options.clone());
            }
            let mut closed = StoredEvent {
                sequence: facts[3].sequence + 1,
                event: facts[3].event.clone(),
            };
            closed.event.id = "part-closed".into();
            closed.event.fact = Fact::ModelObserved {
                step_id: "step".into(),
                event: ModelEvent::PartFinished {
                    id: "provider-part".into(),
                    provider_options: None,
                },
            };
            facts.insert(4, closed);
            let mut view = InvocationView::new(1024).unwrap();
            for fact in facts {
                for row in view.push(&fact).unwrap() {
                    if let Content::Assistant {
                        interrupted, text, ..
                    } = &row.message.content
                    {
                        assert_eq!(*interrupted, kind == TextKind::Text || !finalized);
                        assert_eq!(text.is_empty(), kind == TextKind::Thinking);
                        messages.push(row.message);
                    }
                }
            }
        }
    }
    // Same core decoder used by Desktop, not just a permissive JSON assembler.
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/support/source.mjs"))
        .arg("crates/presentation/tests/fixtures/messages.mjs")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&messages).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn malformed_or_oversized_evidence_poison_the_view_instead_of_publishing_partial_success() {
    let mut events = events(InvocationOutcome::Completed);
    let mut view = InvocationView::new(64).unwrap();
    for event in &events[..3] {
        view.push(event).unwrap();
    }
    let Fact::ModelObserved {
        event: ModelEvent::PartDelta { text, .. },
        ..
    } = &mut events[3].event.fact
    else {
        unreachable!()
    };
    *text = "x".repeat(65);
    assert!(matches!(
        view.push(&events[3]),
        Err(ProjectionError::TooLarge)
    ));
    assert!(view.overlay().is_empty(), "discard poisoned projection");
    assert!(
        view.push(events.last().unwrap()).is_err(),
        "cannot seal after discarded evidence"
    );

    let mut view = InvocationView::new(1024).unwrap();
    view.push(&events[0]).unwrap();
    assert!(
        view.push(&events[0]).is_err(),
        "sequence replay is not another message"
    );
    assert!(watermark(9_007_199_254_740_991).is_err());
}
