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
use maka_runtime::{
    continuation::{MAX_SOURCE_BYTES, MAX_SOURCE_EVENTS},
    handoff::{HandoffIntent, HandoffPause},
    model::{ModelFinishReason, ModelPart, ModelStep, ModelUsage, TextKind},
};
use std::{
    num::NonZeroU16,
    time::{Duration, UNIX_EPOCH},
};

#[tokio::test]
async fn handoff_reserves_both_envelopes_before_sealing_a_near_capacity_context() {
    for reserve_successor in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let log = EventLog::open(&directory.path().join("capacity.sqlite"))
            .await
            .unwrap();
        let source = opening("root", None);
        append(&log, &source).await;
        let context = log
            .read_model_context("session", Some("root"), 100, 65536)
            .await
            .unwrap();
        append(
            &log,
            &RuntimeEvent::new(
                source.invocation.clone(),
                Fact::ModelRequested {
                    step_id: "step".into(),
                    model_id: "test".into(),
                    purpose: Some(maka_runtime::context::ModelPurpose::Main),
                    context: None,
                    source_scope: context.source_evidence.scope,
                    source_high_water: context.source_evidence.high_water,
                    source_digest: context.source_evidence.digest,
                    effective_source_digest: Some(context.effective_source_digest),
                    input_digest: digest('a'),
                    route_identity: digest('b'),
                    checkpoint_event_id: None,
                },
            ),
        )
        .await;
        let prefix = log
            .run_prefix("session", "root", None, 100, MAX_SOURCE_BYTES)
            .await
            .unwrap()
            .unwrap();
        let existing_bytes: usize = prefix
            .events
            .iter()
            .map(|event| serde_json::to_vec(&event.event).unwrap().len())
            .sum();
        let pause = HandoffPause {
            intent: HandoffIntent {
                handoff_id: "handoff".into(),
                host_epoch: "host".into(),
                root_run_id: "root".into(),
                successor_run_id: "successor-run".into(),
                successor_invocation_id: "successor-invocation".into(),
                claim_id: "claim".into(),
            },
            remaining_steps: NonZeroU16::new(2).unwrap(),
            execution: super::handoff::execution(),
        };
        let mut seal = RuntimeEvent::new(
            source.invocation.clone(),
            Fact::InvocationEnded {
                outcome: InvocationOutcome::HandoffPaused {
                    pause: pause.clone(),
                },
            },
        );
        let mut widest = serde_json::to_value(&seal).unwrap();
        widest["recorded_at"] =
            serde_json::json!({ "secs_since_epoch": u64::MAX, "nanos_since_epoch": u32::MAX });
        let mut reserved_bytes = serde_json::to_vec(&widest).unwrap().len();
        let mut future = serde_json::to_value(&source).unwrap();
        future["invocation"] =
            serde_json::to_value(pause.intent.successor(&source.invocation)).unwrap();
        future["recorded_at"] = widest["recorded_at"].clone();
        future["fact"]["input"] = serde_json::json!({
            "kind":"handoff", "pause": pause,
            "claim": {
                "id":pause.intent.claim_id,
                "source":{"invocation":source.invocation,"high_water":u64::MAX,"digest":digest('a')},
                "base":{"high_water":u64::MAX,"digest":digest('a')},
                "replay":{"version":REPLAY_VERSION,"digest":digest('a'),"route_identity":digest('b')}
            }
        });
        if reserve_successor {
            reserved_bytes += serde_json::to_vec(&future).unwrap().len();
        }
        let mut completed = RuntimeEvent::new(
            source.invocation.clone(),
            Fact::ModelCompleted {
                step_id: "step".into(),
                output: ModelStep {
                    parts: vec![ModelPart::Text {
                        text_kind: TextKind::Text,
                        text: String::new(),
                        provider_options: None,
                    }],
                    finish_reason: ModelFinishReason::Stop,
                    usage: ModelUsage::default(),
                    provider_options: None,
                    response_id: None,
                    model: None,
                    timestamp: None,
                },
            },
        );
        let remaining = MAX_SOURCE_BYTES
            - reserved_bytes
            - existing_bytes
            - serde_json::to_vec(&completed).unwrap().len();
        let Fact::ModelCompleted { output, .. } = &mut completed.fact else {
            unreachable!()
        };
        let ModelPart::Text { text, .. } = &mut output.parts[0] else {
            unreachable!()
        };
        *text = "x".repeat(remaining);
        append(&log, &completed).await;
        if !reserve_successor {
            assert!(
                log.check_handoff(&source.invocation, &pause).await.is_err(),
                "fitting the source seal alone does not make its successor readable"
            );
            assert!(log.append(&EventWrite::plain(seal).unwrap()).await.is_err());
            log.close().await.unwrap();
            continue;
        }
        log.check_handoff(&source.invocation, &pause).await.unwrap();
        seal.recorded_at = UNIX_EPOCH + Duration::new(100_000_000_000, 999_999_999);
        append(&log, &seal).await;
        let sealed = log
            .run_prefix("session", "root", None, MAX_SOURCE_EVENTS, MAX_SOURCE_BYTES)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sealed.events.last().unwrap().event, seal);
        assert_eq!(
            sealed.high_water, 4,
            "the pause remains claimable within the exact source budget"
        );
        let base = log
            .context_before_run(&source.invocation, 100, MAX_SOURCE_BYTES)
            .await
            .unwrap();
        let claim = ContinuationClaim {
            id: pause.intent.claim_id.clone(),
            source: RunBoundary {
                invocation: sealed.invocation,
                high_water: sealed.high_water,
                digest: sealed.digest,
            },
            base: SessionBase {
                high_water: base.source_evidence.high_water,
                digest: base.source_evidence.digest,
            },
            replay: ReplayEvidence {
                version: REPLAY_VERSION,
                digest: digest('a'),
                route_identity: digest('b'),
            },
        };
        let mut target = opening("unused", None);
        target.invocation = pause.intent.successor(&source.invocation);
        let Fact::InvocationOpened { input, .. } = &mut target.fact else {
            unreachable!()
        };
        *input = InvocationInput::Handoff {
            claim: Box::new(claim),
            pause: Box::new(pause),
        };
        append(&log, &target).await;
        log.read_model_context(
            "session",
            Some(&target.invocation.invocation_id),
            MAX_SOURCE_EVENTS,
            MAX_SOURCE_BYTES,
        )
        .await
        .unwrap();
        log.close().await.unwrap();
    }
}
