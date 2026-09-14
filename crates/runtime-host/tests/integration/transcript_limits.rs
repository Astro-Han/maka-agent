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

use maka_event_log::EventLog;
use maka_presentation::watermark;
use maka_protocol::transcript::*;
use maka_runtime::event::{
    EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent,
};
use maka_runtime_host::transcript::Transcript;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[tokio::test]
async fn oversized_turn_uses_null_boundary_without_losing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::open(&dir.path().join("log.sqlite"))
        .await
        .unwrap();
    let mut fence = 0;
    // One logical Turn may have successor Runs. Its 258 rows exceed the range
    // message cap without requiring a giant payload allocation in this test.
    for number in 0..129 {
        let invocation = Invocation {
            session_id: "session".into(),
            turn_id: "turn".into(),
            run_id: format!("run-{number}"),
            invocation_id: format!("inv-{number}"),
        };
        log.append(
            &EventWrite::plain(RuntimeEvent::new(
                invocation.clone(),
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        skill_invocation: Default::default(),
                        source_messages: Vec::new(),
                        content: "text".into(),
                        request_fingerprint: None,
                    },
                },
            ))
            .unwrap(),
        )
        .await
        .unwrap();
        fence = log
            .append(
                &EventWrite::plain(RuntimeEvent::new(
                    invocation,
                    Fact::InvocationEnded {
                        outcome: InvocationOutcome::Completed,
                    },
                ))
                .unwrap(),
            )
            .await
            .unwrap();
    }
    while !log.prepare_transcript("session", fence, 32).await.unwrap() {}
    let through = Some(watermark(fence).unwrap());
    let state = Transcript::new(
        "sub".into(),
        "session".into(),
        through,
        vec![],
        Arc::new(Semaphore::new(1024)),
    )
    .unwrap();
    let mut request = SessionTranscriptPageInput {
        subscription_id: "sub".into(),
        source: SessionTranscriptPageSource::Durable,
        direction: SessionTranscriptPageDirection::Older,
        through_sequence: through,
        cursor: None,
        anchor_sequence: None,
        max_bytes: 524288,
    };
    let first = state.page(&log, &request).await.unwrap();
    assert_eq!(first.fragments.len(), 256);
    assert_eq!(first.range_boundary_sequence, None);
    assert_eq!(first.protected_turn_sequence, None);
    request.cursor = first.next_cursor;
    assert!(request.cursor.is_some());
    let last = state.page(&log, &request).await.unwrap();
    assert_eq!(last.fragments.len(), 2);
    assert_eq!(last.range_boundary_sequence, None);
    assert!(last.next_cursor.is_none());
    assert!(last.fragments[0].identity() < first.fragments.last().unwrap().identity());
}

#[tokio::test]
async fn empty_fresh_tail_is_truthful_and_budgeted() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::open(&dir.path().join("log.sqlite"))
        .await
        .unwrap();
    let state = Transcript::new(
        "sub".into(),
        "session".into(),
        None,
        vec![],
        Arc::new(Semaphore::new(1024)),
    )
    .unwrap();
    let bootstrap = state.bootstrap(&log, 2).await.unwrap();
    assert_eq!(bootstrap.through_sequence, None);
    assert_eq!(bootstrap.overlay_message_count, 0);
    assert_eq!(bootstrap.durable.raw_bytes + bootstrap.overlay.raw_bytes, 0);
    assert!(bootstrap.durable.fragments.is_empty());
    assert!(bootstrap.durable.next_cursor.is_none());
    assert!(state.bootstrap(&log, 1).await.is_err());
}
