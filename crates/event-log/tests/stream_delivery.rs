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

use maka_event_log::{EventLog, StoreError, observation::StreamFact};
use maka_runtime::event::EventWrite;
use maka_runtime::{
    event::{Fact, Invocation, InvocationOutcome, LogScope, RuntimeEvent},
    model::{ModelEvent, ModelPart, ModelStep, TextKind},
};
use serde_json::json;

fn event(session: &str, fact: Fact) -> RuntimeEvent {
    RuntimeEvent::new(
        Invocation {
            session_id: session.into(),
            turn_id: format!("turn-{session}"),
            run_id: format!("run-{session}"),
            invocation_id: format!("invocation-{session}"),
        },
        fact,
    )
}

fn observed(step: &str, observation: ModelEvent) -> Fact {
    Fact::ModelObserved {
        step_id: step.into(),
        event: observation,
    }
}

fn request(step: &str) -> Fact {
    Fact::ModelRequested {
        purpose: maka_runtime::context::ModelPurpose::Main,
        context: None,
        checkpoint_event_id: None,
        step_id: step.into(),
        model_id: "test".into(),
        source_scope: LogScope::Root,
        source_high_water: 0,
        source_digest: "fixture".into(),
        effective_source_digest: None,
        input_digest: "input".into(),
        route_identity: "route".into(),
    }
}

fn delta(text: &str) -> Fact {
    observed(
        "second",
        ModelEvent::PartDelta {
            id: "part".into(),
            text: text.into(),
            provider_options: None,
        },
    )
}

#[tokio::test]
async fn giant_bodies_and_metadata_do_not_block_stream_delivery() {
    let temp = tempfile::tempdir().unwrap();
    let log = EventLog::open(&temp.path().join("events.sqlite"))
        .await
        .unwrap();
    let append = async |fact| {
        log.append(&EventWrite::plain((event("session", fact)).clone()).unwrap())
            .await
            .unwrap()
    };
    let opened = append(Fact::InvocationOpened {
        configuration: None,
        input: maka_runtime::input::InvocationInput::Message {
            source_messages: Vec::new(),
            content: "".into(),
            request_fingerprint: None,
        },
    })
    .await;
    append(request("first")).await;
    let giant = "x".repeat(2 * 1024 * 1024);
    let completed = append(Fact::ModelCompleted {
        step_id: "first".into(),
        output: ModelStep {
            parts: vec![ModelPart::Text {
                text_kind: TextKind::Text,
                text: giant.clone(),
                provider_options: Some(json!({"large": giant})),
            }],
            finish_reason: maka_runtime::model::ModelFinishReason::Stop,
            usage: Default::default(),
            provider_options: None,
            response_id: None,
            model: None,
            timestamp: None,
        },
    })
    .await;
    let steered = append(Fact::MessageSteered {
        message: Box::new(maka_runtime::input::DeliveredMessage {
            message_id: "steering".into(),
            content: "s".repeat(64 * 1024).into(),
            submitted_content_digest: format!("sha256:{}", "b".repeat(64)),
        }),
    })
    .await;
    append(request("second")).await;
    append(observed(
        "second",
        ModelEvent::ProviderToolResult {
            id: "unknown-provider-tool".into(),
            name: "provider-internal".into(),
            output: json!({"large": giant}),
            is_error: false,
            provider_options: None,
        },
    ))
    .await;
    let start = event(
        "session",
        observed(
            "second",
            ModelEvent::PartStarted {
                id: "part".into(),
                text_kind: TextKind::Thinking,
                provider_options: Some(json!({"large": giant})),
            },
        ),
    );
    let started = log
        .append(&EventWrite::plain((start).clone()).unwrap())
        .await
        .unwrap();
    let delta_sequence = append(observed(
        "second",
        ModelEvent::PartDelta {
            id: "part".into(),
            text: "a😀\n\"中".into(),
            provider_options: Some(json!({"large": giant})),
        },
    ))
    .await;
    let finished = append(observed(
        "second",
        ModelEvent::PartFinished {
            id: "part".into(),
            provider_options: Some(json!({"large": giant})),
        },
    ))
    .await;
    let interrupted = append(Fact::ModelInterrupted {
        step_id: "second".into(),
        status: maka_runtime::event::ModelInterruption::Failed,
    })
    .await;
    let terminal = append(Fact::InvocationEnded {
        outcome: InvocationOutcome::Failed {
            class: "provider".into(),
            message: None,
        },
    })
    .await;
    let page = log
        .session_stream_events("session", 0, terminal, 512, 4096)
        .await
        .unwrap();
    assert_eq!(
        page.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        [
            opened,
            completed,
            steered,
            started,
            delta_sequence,
            finished,
            interrupted,
            terminal
        ]
    );
    assert_eq!(page.events[3].id, start.id);
    assert_eq!(page.events[3].invocation, start.invocation);
    assert_eq!(
        page.events[4].fact,
        StreamFact::PartDelta {
            step_id: "second".into(),
            part_id: "part".into(),
            text: "a😀\n\"中".into()
        }
    );
    assert!(matches!(page.events[0].fact, StreamFact::InvocationOpened));
    assert!(matches!(page.events[1].fact, StreamFact::StepEnded { .. }));
    assert!(matches!(
        page.events[5].fact,
        StreamFact::PartFinished { .. }
    ));
    assert!(matches!(page.events[2].fact, StreamFact::MessageSteered));
    assert!(matches!(
        page.events[7].fact,
        StreamFact::InvocationEnded { .. }
    ));
    assert_eq!(page.through_sequence, terminal);
    assert_eq!(page.next_after, None);
    assert!(matches!(&page.events[6].fact,
        StreamFact::StepEnded { failed: true, interrupted, .. }
        if interrupted == std::slice::from_ref(&start.id)));
    assert!(matches!(&page.events[7].fact,
        StreamFact::InvocationEnded { failed: true, interrupted }
        if interrupted.is_empty()));
    // The derived interruption identities belong to the delivery byte budget.
    let bytes = serde_json::to_vec(&page.events[6]).unwrap().len();
    assert!(matches!(
        log.session_stream_events("session", finished, interrupted, 1, bytes - 1)
            .await,
        Err(StoreError::PrefixTooLarge)
    ));
    let exact = log
        .session_stream_events("session", finished, interrupted, 1, bytes)
        .await
        .unwrap();
    assert_eq!(exact.events, page.events[6..7]);
    assert_eq!(exact.next_after, None);
}

#[tokio::test]
async fn pages_obey_global_fences_and_never_skip_oversized_deltas() {
    let temp = tempfile::tempdir().unwrap();
    let log = EventLog::open(&temp.path().join("events.sqlite"))
        .await
        .unwrap();
    let opened = log
        .append(
            &EventWrite::plain(
                (event(
                    "session",
                    Fact::InvocationOpened {
                        configuration: None,
                        input: maka_runtime::input::InvocationInput::Message {
                            source_messages: Vec::new(),
                            content: "".into(),
                            request_fingerprint: None,
                        },
                    },
                ))
                .clone(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    log.append(&EventWrite::plain((event("session", request("second"))).clone()).unwrap())
        .await
        .unwrap();
    let first = log
        .append(&EventWrite::plain((event("session", delta("😀"))).clone()).unwrap())
        .await
        .unwrap();
    let gap = log
        .append(
            &EventWrite::plain(
                (event(
                    "other",
                    Fact::InvocationOpened {
                        configuration: None,
                        input: maka_runtime::input::InvocationInput::Message {
                            source_messages: Vec::new(),
                            content: "".into(),
                            request_fingerprint: None,
                        },
                    },
                ))
                .clone(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let large = log
        .append(&EventWrite::plain((event("session", delta(&"中".repeat(4096)))).clone()).unwrap())
        .await
        .unwrap();
    let last = log
        .append(&EventWrite::plain((event("session", delta("last"))).clone()).unwrap())
        .await
        .unwrap();
    let old = log
        .session_stream_events("session", opened, gap, 1, 1024)
        .await
        .unwrap();
    assert_eq!(old.events.len(), 1);
    assert_eq!(old.events[0].sequence, first);
    assert_eq!(old.next_after, None);
    assert_eq!(old.through_sequence, gap);
    let limited = log
        .session_stream_events("session", opened, last, 512, 1024)
        .await
        .unwrap();
    assert_eq!(limited.events.len(), 1);
    assert_eq!(limited.next_after, Some(first));
    assert!(matches!(
        log.session_stream_events("session", first, last, 512, 1024)
            .await,
        Err(StoreError::PrefixTooLarge)
    ));
    let next = log
        .session_stream_events("session", first, last, 1, 32 * 1024)
        .await
        .unwrap();
    assert_eq!(next.events[0].sequence, large);
    assert_eq!(next.next_after, Some(large));
    let tail = log
        .session_stream_events("session", large, last, 1, 1024)
        .await
        .unwrap();
    assert_eq!(tail.events[0].sequence, last);
    assert_eq!(tail.next_after, None);
    for (after, through, count, bytes) in [
        (0, last + 1, 1, 1024),
        (last, 0, 1, 1024),
        (0, last, 0, 1024),
        (0, last, 513, 1024),
        (0, last, 1, 0),
    ] {
        assert!(
            log.session_stream_events("session", after, through, count, bytes)
                .await
                .is_err()
        );
    }
}
