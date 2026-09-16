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

use base64::{Engine, engine::general_purpose::STANDARD};
use maka_event_log::EventLog;
use maka_presentation::watermark;
use maka_protocol::transcript::*;
use maka_runtime::event::{
    EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent,
};
use maka_runtime_host::transcript::Transcript;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

async fn fixture() -> (tempfile::TempDir, EventLog, u64) {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::open(&dir.path().join("log.sqlite"))
        .await
        .unwrap();
    let mut fence = 0;
    for number in 0..2 {
        let invocation = Invocation {
            session_id: "session".into(),
            turn_id: format!("turn-{number}"),
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
                        content: "你好😀".into(),
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
    assert!(log.prepare_transcript("session", fence, 32).await.unwrap());
    (dir, log, watermark(fence).unwrap())
}
fn pager(id: &str, through: u64) -> Transcript {
    Transcript::new(id.into(), "session".into(), Some(through)).unwrap()
}
fn request(
    through: u64,
    direction: SessionTranscriptPageDirection,
    max_bytes: u64,
) -> SessionTranscriptPageInput {
    SessionTranscriptPageInput {
        subscription_id: "sub".into(),
        direction,
        through_sequence: Some(through),
        cursor: None,
        anchor_sequence: None,
        max_bytes,
    }
}

#[tokio::test]
async fn byte_fragments_reassemble_both_directions_with_digest_and_reachable_turn_edges() {
    let (_dir, log, through) = fixture().await;
    let state = pager("sub", through);
    let mut expected = None;
    for direction in [
        SessionTranscriptPageDirection::Older,
        SessionTranscriptPageDirection::Newer,
    ] {
        let mut input = request(through, direction, 1);
        let mut messages: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
        let mut offsets = BTreeMap::new();
        let mut digests = BTreeMap::new();
        let mut boundary = None;
        let mut completed_ranges = 0;
        loop {
            let page = state.page(&log, &input).await.unwrap();
            assert_eq!(page.raw_bytes, 1);
            assert_eq!(page.fragments.len(), 1);
            if boundary.is_none() {
                boundary = page.range_boundary_sequence;
            }
            assert_eq!(page.range_boundary_sequence, boundary);
            let SessionTranscriptFragment {
                sequence,
                byte_offset,
                total_bytes,
                payload_digest,
                data,
            } = &page.fragments[0];
            let bytes = STANDARD.decode(data).unwrap();
            let buffer = messages
                .entry(*sequence)
                .or_insert_with(|| vec![0; *total_bytes as usize]);
            let previous = offsets.entry(*sequence).or_insert(
                if direction == SessionTranscriptPageDirection::Older {
                    *total_bytes
                } else {
                    0
                },
            );
            if direction == SessionTranscriptPageDirection::Older {
                assert_eq!(byte_offset + bytes.len() as u64, *previous);
                *previous = *byte_offset;
            } else {
                assert_eq!(*byte_offset, *previous);
                *previous += bytes.len() as u64;
            }
            buffer[*byte_offset as usize..*byte_offset as usize + bytes.len()]
                .copy_from_slice(&bytes);
            assert_eq!(
                digests.entry(*sequence).or_insert(payload_digest.clone()),
                payload_digest
            );
            let complete = if direction == SessionTranscriptPageDirection::Older {
                *previous == 0
            } else {
                *previous == *total_bytes
            };
            if complete && boundary == Some(*sequence) {
                boundary = None;
                completed_ranges += 1;
            }
            input.cursor = page.next_cursor;
            if input.cursor.is_none() {
                break;
            }
        }
        assert_eq!(completed_ranges, 2);
        assert_eq!(messages.len(), 4);
        for (sequence, bytes) in &messages {
            assert_eq!(
                digests[sequence],
                Some(format!("sha256:{:x}", Sha256::digest(bytes)))
            );
            let _: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        }
        if let Some(expected) = &expected {
            assert_eq!(&messages, expected);
        } else {
            expected = Some(messages);
        }
    }
}

#[tokio::test]
async fn anchors_are_exclusive_and_cursors_reject_tampering_and_transplants() {
    let (_dir, log, through) = fixture().await;
    let state = pager("sub", through);
    let mut input = request(through, SessionTranscriptPageDirection::Newer, 512 * 1024);
    let first = state.page(&log, &input).await.unwrap();
    assert_eq!(first.fragments.len(), 2);
    let first_sequence = first.fragments[0].identity();
    input.anchor_sequence = Some(first_sequence);
    let next = state.page(&log, &input).await.unwrap();
    assert!(next.fragments.iter().all(|f| f.identity() > first_sequence));
    input.direction = SessionTranscriptPageDirection::Older;
    assert!(state.page(&log, &input).await.unwrap().fragments.is_empty());
    input.anchor_sequence = None;
    input.max_bytes = 1;
    input.cursor = state.page(&log, &input).await.unwrap().next_cursor;
    assert!(pager("sub", through).page(&log, &input).await.is_err());
    input.direction = SessionTranscriptPageDirection::Newer;
    assert!(state.page(&log, &input).await.is_err());
    input.direction = SessionTranscriptPageDirection::Older;
    input.through_sequence = Some(through - 1);
    assert!(state.page(&log, &input).await.is_err());
    input.through_sequence = Some(through);
    let token = input.cursor.as_mut().unwrap();
    token.replace_range(0..1, if token.starts_with('a') { "b" } else { "a" });
    assert!(state.page(&log, &input).await.is_err());
}

#[tokio::test]
async fn bootstrap_and_pages_share_the_announced_log_fence() {
    let (_dir, log, through) = fixture().await;
    let mut state = pager("sub", through);
    let bootstrap = state.bootstrap(&log, 2).await.unwrap();
    assert_eq!(bootstrap.durable.raw_bytes, 2);
    let mut input = request(through + 256, SessionTranscriptPageDirection::Older, 1);
    assert!(state.page(&log, &input).await.is_err());
    assert!(state.advance(Some(through + 256)).unwrap());
    assert!(!state.advance(Some(through + 256)).unwrap());
    assert!(state.advance(Some(through)).is_err());
    input.through_sequence = Some(through);
    assert!(state.page(&log, &input).await.is_ok());
}
