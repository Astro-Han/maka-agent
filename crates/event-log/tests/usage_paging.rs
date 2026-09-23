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

use maka_event_log::{EventLog, usage::Query};
use maka_runtime::{
    accounting::{Activity, Selection},
    context::ModelPurpose,
    event::{
        EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, LogScope, RuntimeEvent,
    },
    model::{ModelFinishReason, ModelStep, ModelUsage},
};
use std::{
    collections::BTreeSet,
    time::{Duration, UNIX_EPOCH},
};

#[tokio::test]
async fn accounting_pages_bound_bytes_without_dropping_records_or_reading_response_bodies() {
    let directory = tempfile::tempdir().unwrap();
    let log = EventLog::open(&directory.path().join("usage.sqlite"))
        .await
        .unwrap();
    log.append(&event(Fact::InvocationOpened {
        configuration: None,
        input: InvocationInput::Message {
            content: "not Usage data".into(),
            source_messages: vec![],
            request_fingerprint: None,
        },
    }))
    .await
    .unwrap();
    // Long public model IDs reach the byte bound before the item bound.
    for i in 0..80 {
        log.append_batch(&[
            event(Fact::ModelRequested {
                step_id: format!("step-{i}"),
                model_id: format!("{i}-{}", "model".repeat(400)),
                purpose: ModelPurpose::Main,
                source_scope: LogScope::Session { id: "usage".into() },
                source_high_water: 0,
                source_digest: "source".into(),
                input_digest: "input".into(),
                route_identity: "route".into(),
                checkpoint_event_id: None,
                context: None,
                effective_source_digest: None,
            }),
            event(Fact::ModelCompleted {
                step_id: format!("step-{i}"),
                output: ModelStep {
                    model: None,
                    response_id: None,
                    timestamp: None,
                    parts: vec![maka_runtime::model::ModelPart::Text {
                        text_kind: maka_runtime::model::TextKind::Text,
                        text: "x".repeat(65536),
                        provider_options: None,
                    }],
                    finish_reason: ModelFinishReason::Stop,
                    usage: ModelUsage {
                        input_tokens: Some(i),
                        ..Default::default()
                    },
                    provider_options: None,
                },
            }),
        ])
        .await
        .unwrap();
    }
    log.append(&event(Fact::InvocationEnded {
        outcome: InvocationOutcome::Completed,
    }))
    .await
    .unwrap();
    let mut query = Query {
        from: 10.25,
        to: 10.25,
        session_id: None,
        through: None,
    };
    let mut offset = 0;
    let mut seen = BTreeSet::new();
    loop {
        let page = log
            .usage_activity(query.clone(), Selection::default(), offset, 100)
            .await
            .unwrap();
        assert_eq!(page.total, 80);
        assert!(!page.attempts.is_empty() && page.attempts.len() < 80);
        assert!(serde_json::to_vec(&page.attempts).unwrap().len() <= 32 * 1024);
        for activity in page.attempts {
            let Activity::Model(attempt) = activity else {
                panic!("unexpected tool activity")
            };
            assert!(seen.insert(attempt.request_id));
            assert_eq!(attempt.session_id.as_deref(), Some("usage"));
        }
        query.through = Some(page.through);
        match page.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(seen.len(), 80);
    assert!(
        matches!(log.usage_summary(query).await,
        Err(maka_event_log::StoreError::InvalidTransition(message))
            if message.contains("response capacity")),
        "oversized complete breakdowns fail rather than silently dropping groups"
    );
    log.close().await.unwrap();
}

fn event(fact: Fact) -> EventWrite {
    let mut event = RuntimeEvent::new(
        Invocation {
            session_id: "usage".into(),
            turn_id: "turn".into(),
            run_id: "run".into(),
            invocation_id: "invocation".into(),
        },
        fact,
    );
    event.recorded_at = UNIX_EPOCH + Duration::from_micros(10250);
    EventWrite::plain(event).unwrap()
}
