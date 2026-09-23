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
async fn search_walks_bounded_batches_and_binds_cursors_to_query_subscription_and_fence() {
    let (_dir, log, through) = fixture().await;
    let mut state = pager("sub", through);
    let mut input = TranscriptSearchInput {
        subscription_id: "sub".into(),
        through_sequence: Some(through),
        query: "你好".into(),
        include_internal: false,
        cursor: None,
        max_matches: 1,
    };
    let first = state.search(&log, &input).await.unwrap();
    assert_eq!(first.matches.len(), 1);
    assert!(first.matches[0].preview.contains("你好😀"));
    input.cursor = first.next_cursor.clone();
    assert!(input.cursor.is_some());
    let second = state.search(&log, &input).await.unwrap();
    assert_eq!(second.matches.len(), 1);
    assert!(second.matches[0].sequence > first.matches[0].sequence);
    for kind in ["query", "visibility", "fence", "subscription", "tamper"] {
        let mut changed = input.clone();
        match kind {
            "query" => changed.query = "😀".into(),
            "visibility" => changed.include_internal = true,
            "fence" => changed.through_sequence = Some(through - 1),
            "subscription" => changed.subscription_id = "other".into(),
            _ => changed.cursor.as_mut().unwrap().push('a'),
        }
        assert!(state.search(&log, &changed).await.is_err(), "{kind}");
    }
    assert!(pager("sub", through).search(&log, &input).await.is_err());
    let mut page_input = request(through, SessionTranscriptPageDirection::Newer, 32);
    page_input.cursor = input.cursor.clone();
    assert!(
        state.page(&log, &page_input).await.is_err(),
        "search cursor is not a page cursor"
    );
    input.cursor = None;
    input.through_sequence = Some(through + 256);
    assert!(
        state.search(&log, &input).await.is_err(),
        "unannounced history"
    );
    input.through_sequence = Some(through);
    let mut fence = 0;
    for n in 0..40 {
        let invocation = Invocation {
            session_id: "session".into(),
            turn_id: format!("late-{n}"),
            run_id: format!("late-{n}"),
            invocation_id: format!("late-{n}"),
        };
        log.append(
            &EventWrite::plain(RuntimeEvent::new(
                invocation.clone(),
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        source_messages: vec![],
                        content: format!("later 你好 {n}").into(),
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
    let new_through = watermark(fence).unwrap();
    state.advance(Some(new_through)).unwrap();
    input.max_matches = 64;
    let frozen = state.search(&log, &input).await.unwrap();
    assert_eq!(
        frozen.matches.len(),
        2,
        "new commits cannot enter an old search fence"
    );
    assert!(frozen.next_cursor.is_none());
    input.through_sequence = Some(new_through);
    input.query = "not found anywhere".into();
    let empty_batch = state.search(&log, &input).await.unwrap();
    assert!(empty_batch.matches.is_empty());
    input.cursor = empty_batch.next_cursor;
    assert!(
        input.cursor.is_some(),
        "zero matches does not imply the scan completed"
    );
    let last = state.search(&log, &input).await.unwrap();
    assert!(last.matches.is_empty() && last.next_cursor.is_none());
}
