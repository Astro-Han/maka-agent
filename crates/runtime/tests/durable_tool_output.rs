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

use maka_runtime::{
    capability::{CallResult, ContentBlock},
    event::{CommitError, CommitFuture, EventSink, EventWrite, Fact, Invocation, ToolOutcome},
    tool_call::ToolCallIdentity,
    tool_output::{DurableToolProjection, ToolOutput, ToolSuccess, decode_raw_tool_result},
    tools::{PreparedEffect, ToolError, ToolJournal},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
fn invocation() -> Invocation {
    Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    }
}
fn write(value: &ToolOutput) -> EventWrite {
    EventWrite::tool_success(
        "outcome".into(),
        UNIX_EPOCH + Duration::from_secs(17),
        invocation(),
        "operation".into(),
        value.clone().into(),
    )
    .unwrap()
    .0
}
fn projection(write: &EventWrite) -> &DurableToolProjection {
    let Fact::ToolSettled {
        outcome: ToolOutcome::Succeeded {
            model_projection, ..
        },
        ..
    } = &write.event().fact
    else {
        panic!()
    };
    model_projection
}
#[test]
fn raw_depth_roundtrip_is_bounded_for_every_embedded_json_field() {
    use maka_runtime::{artifact::content_digest, tool_output::RawToolResultRef};
    for depth in [33, 64, 65, 130] {
        let mut deep = Value::Null;
        for level in 0..depth {
            deep = if level % 2 == 0 {
                json!([deep])
            } else {
                json!({"nested":deep})
            };
        }
        for output in [
            ToolOutput::Json(deep.clone()),
            ToolOutput::Mcp(CallResult {
                content: vec![],
                structured_content: Some(deep.clone()),
            }),
            ToolOutput::Mcp(CallResult {
                content: vec![ContentBlock::Unknown { value: deep }],
                structured_content: None,
            }),
        ] {
            let written = EventWrite::tool_success(
                "outcome".into(),
                UNIX_EPOCH,
                invocation(),
                "operation".into(),
                output.clone().into(),
            );
            if depth <= 64 {
                let written = written.unwrap().0;
                let Fact::ToolSettled {
                    outcome: ToolOutcome::Succeeded { raw, .. },
                    ..
                } = &written.event().fact
                else {
                    panic!()
                };
                assert_eq!(
                    decode_raw_tool_result(written.raw_payload().unwrap(), raw).unwrap(),
                    output
                );
            } else {
                assert!(
                    matches!(written, Err(CommitError::Rejected(message)) if message.contains("depth limit"))
                );
                let bytes = serde_json::to_vec(&output).unwrap();
                let evidence = RawToolResultRef {
                    bytes: bytes.len() as u64,
                    digest: content_digest(&bytes),
                };
                assert!(decode_raw_tool_result(&bytes, &evidence).is_err());
            }
        }
    }
}
#[test]
fn raw_ten_mib_control_text_roundtrips_while_durable_fact_stays_small() {
    for unit in ["a", "🦊", "\u{0000}"] {
        let raw = ToolOutput::Text(unit.repeat(10 * 1024 * 1024 / unit.len()));
        let write = write(&raw);
        assert_eq!(write.event().id, "outcome");
        assert_eq!(
            write.event().recorded_at,
            UNIX_EPOCH + Duration::from_secs(17)
        );
        assert_eq!(projection(&write), &DurableToolProjection::Failure);
        assert!(serde_json::to_vec(write.event()).unwrap().len() < 1024);
        let Fact::ToolSettled {
            outcome: ToolOutcome::Succeeded { raw: evidence, .. },
            ..
        } = &write.event().fact
        else {
            panic!()
        };
        let bytes = write.raw_payload().unwrap();
        assert_eq!(decode_raw_tool_result(bytes, evidence).unwrap(), raw);
        let mut bad = evidence.clone();
        bad.bytes += 1;
        assert!(decode_raw_tool_result(bytes, &bad).is_err());
        bad = evidence.clone();
        bad.digest.push('0');
        assert!(decode_raw_tool_result(bytes, &bad).is_err());
        assert!(EventWrite::plain(write.event().clone()).is_err());
    }
}

#[test]
fn projection_limits_cover_bytes_depth_nodes_and_parts_without_losing_raw() {
    let limit = 256 * 1024;
    let empty = serde_json::to_vec(&DurableToolProjection::Text {
        text: String::new(),
    })
    .unwrap()
    .len();
    assert!(matches!(
        projection(&write(&ToolOutput::Json(json!("x".repeat(limit - empty))))),
        DurableToolProjection::Text { .. }
    ));
    assert_eq!(
        projection(&write(&ToolOutput::Json(json!(
            "x".repeat(limit - empty + 1)
        )))),
        &DurableToolProjection::Failure
    );
    let mut value = Value::Null;
    for _ in 0..32 {
        value = json!([value]);
    }
    assert!(matches!(
        projection(&write(&ToolOutput::Json(value.clone()))),
        DurableToolProjection::Json { .. }
    ));
    assert_eq!(
        projection(&write(&ToolOutput::Json(json!([value])))),
        &DurableToolProjection::Failure
    );
    for (nodes, valid) in [(19_999, true), (20_000, false)] {
        assert_eq!(
            !matches!(
                projection(&write(&ToolOutput::Json(json!(vec![0; nodes])))),
                DurableToolProjection::Failure
            ),
            valid
        );
    }
    for (parts, valid) in [(64, true), (65, false)] {
        let raw = ToolOutput::Mcp(CallResult {
            content: vec![ContentBlock::Text { text: "".into() }; parts],
            structured_content: None,
        });
        assert_eq!(
            !matches!(projection(&write(&raw)), DurableToolProjection::Failure),
            valid
        );
    }
    assert!(
        matches!(projection(&write(&ToolOutput::Text("short".into()))), DurableToolProjection::Json { value } if value == &json!({"kind":"text","text":"short"}))
    );
    let invalid_image = ToolOutput::Mcp(CallResult {
        content: vec![ContentBlock::Image {
            data: "bad base64".into(),
            mime_type: "image/png".into(),
        }],
        structured_content: None,
    });
    let write = write(&invalid_image);
    assert_eq!(projection(&write), &DurableToolProjection::Failure);
    assert!(write.projection_artifacts().is_empty());
    assert!(write.raw_payload().is_some());

    // Executor-owned views obey the same bounds without erasing a successful effect.
    let raw = ToolOutput::Text("complete original result".into());
    let (write, delivered) = EventWrite::tool_success(
        "outcome".into(),
        UNIX_EPOCH,
        invocation(),
        "operation".into(),
        ToolSuccess::projected(
            raw.clone(),
            DurableToolProjection::Text {
                text: "x".repeat(limit),
            },
        ),
    )
    .unwrap();
    assert_eq!(projection(&write), &DurableToolProjection::Failure);
    assert_eq!(delivered, raw);
    assert_eq!(
        write.raw_payload().unwrap(),
        serde_json::to_vec(&raw).unwrap()
    );
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    Dispatch,
    OutcomeAck,
}
struct Sink {
    fault: Fault,
    events: Mutex<Vec<EventWrite>>,
    leases: Arc<AtomicUsize>,
}
impl EventSink for Sink {
    fn commit(self: Arc<Self>, write: EventWrite) -> CommitFuture {
        Box::pin(async move {
            if matches!(self.fault, Fault::Dispatch) {
                return Err(CommitError::Rejected("T1 rejected".into()));
            }
            let outcome = matches!(write.event().fact, Fact::ToolSettled { .. });
            assert_eq!(self.leases.load(Ordering::SeqCst), usize::from(outcome));
            let mut events = self.events.lock().unwrap();
            events.push(write);
            if outcome && matches!(self.fault, Fault::OutcomeAck) {
                return Err(CommitError::OutcomeUnknown("ack lost".into()));
            }
            Ok(events.len() as u64)
        })
    }
}

#[tokio::test]
async fn journal_effect_runs_once_and_raw_is_delivered_only_after_t2_ack() {
    struct Lease(Arc<AtomicUsize>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    for fault in [Fault::None, Fault::Dispatch, Fault::OutcomeAck] {
        let leases = Arc::new(AtomicUsize::new(0));
        let admitted = leases.clone();
        let sink = Arc::new(Sink {
            fault,
            events: Mutex::new(Vec::new()),
            leases: leases.clone(),
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let raw = json!({"unchanged":"🦊", "large": "x".repeat(300_000)});
        let input: maka_runtime::read::ReadInput =
            serde_json::from_value(json!({"path":"maka://runtime/attachments/item"})).unwrap();
        let page = input
            .resolve()
            .unwrap()
            .page(raw["large"].as_str().unwrap())
            .unwrap();
        let expected = DurableToolProjection::Json {
            value: serde_json::to_value(page).unwrap(),
        };
        let output = ToolSuccess::projected(ToolOutput::Json(raw.clone()), expected.clone());
        let result = ToolJournal::new(sink.clone(), invocation())
            .invoke_prepared_call(
                "operation".into(),
                ToolCallIdentity::standalone("call".into()),
                "tool".into(),
                Value::Null,
                CancellationToken::new(),
                PreparedEffect::new(move |_| {
                    Box::pin(async move {
                        seen.fetch_add(1, Ordering::SeqCst);
                        Ok(output)
                    })
                })
                .guarded(move || {
                    admitted.fetch_add(1, Ordering::SeqCst);
                    Ok(Lease(admitted))
                }),
            )
            .await;
        assert_eq!(leases.load(Ordering::SeqCst), 0);
        match fault {
            Fault::None => assert_eq!(result.unwrap(), raw),
            Fault::Dispatch => {
                assert!(matches!(result, Err(ToolError::Persistence(_))));
                assert_eq!(calls.load(Ordering::SeqCst), 0);
                continue;
            }
            Fault::OutcomeAck => assert!(matches!(result, Err(ToolError::OutcomeUnknown(_)))),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(projection(&events[1]), &expected);
        let Fact::ToolSettled {
            outcome: ToolOutcome::Succeeded { raw: evidence, .. },
            ..
        } = &events[1].event().fact
        else {
            panic!()
        };
        assert_eq!(
            decode_raw_tool_result(events[1].raw_payload().unwrap(), evidence)
                .unwrap()
                .into_json(),
            raw
        );
    }
}
