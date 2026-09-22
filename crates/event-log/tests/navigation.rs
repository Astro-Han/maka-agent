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

use maka_event_log::{EventLog, StoreError};
use maka_presentation::navigation::{TurnContribution, TurnStatus};
use maka_runtime::event::{
    EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent,
};
use serde_json::json;

fn event(run: &str, turn: &str, fact: Fact) -> EventWrite {
    EventWrite::plain(RuntimeEvent::new(
        Invocation {
            session_id: "session".into(),
            turn_id: turn.into(),
            run_id: run.into(),
            invocation_id: run.into(),
        },
        fact,
    ))
    .unwrap()
}
async fn open(log: &EventLog, run: &str, turn: &str, text: &str) -> u64 {
    log.append(&event(
        run,
        turn,
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: text.into(),
                request_fingerprint: None,
                source_messages: Vec::new(),
            },
        },
    ))
    .await
    .unwrap()
}
async fn end(log: &EventLog, run: &str, turn: &str) -> u64 {
    log.append(&event(
        run,
        turn,
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    ))
    .await
    .unwrap()
}
async fn prepare(log: &EventLog, raw: u64) {
    while !log.prepare_transcript("session", raw, 32).await.unwrap() {}
}
async fn pages(log: &EventLog, through: u64, limit: usize) -> Vec<TurnContribution> {
    let mut position = 0;
    let mut all = Vec::new();
    loop {
        let page = log
            .navigation_turns("session", through, position, limit)
            .await
            .unwrap();
        let size = serde_json::to_vec(&json!({"sessionId":"session","throughSequence":through,
            "contributions":page.contributions,"nextPosition":page.next_position}))
        .unwrap()
        .len();
        assert!(size <= 192 * 1024, "{size}");
        all.extend(page.contributions);
        match page.next_position {
            Some(next) => {
                assert!(next > position);
                position = next;
            }
            None => break,
        }
    }
    all
}

#[tokio::test]
async fn interleaved_turns_keep_full_extents_and_do_not_create_false_page_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let log = EventLog::open(&temp.path().join("events.sqlite"))
        .await
        .unwrap();
    log.create_session("session", "fingerprint", &json!({}), 1)
        .await
        .unwrap();
    let first = open(&log, "parent", "parent", "parent").await;
    end(&log, "parent", "parent").await;
    let child = open(&log, "child", "child", "child").await;
    let child_end = end(&log, "child", "child").await;
    open(&log, "parent-continuation", "parent", "continued").await;
    let last = end(&log, "parent-continuation", "parent").await;
    prepare(&log, last).await;
    let fence = maka_presentation::watermark(last).unwrap();
    for cut in [first * 256, child * 256, child_end * 256] {
        assert!(
            !log.transcript_between_turns("session", fence, cut)
                .await
                .unwrap()
        );
    }
    assert!(
        log.transcript_between_turns("session", fence, last * 256)
            .await
            .unwrap()
    );
    let parent = log
        .navigation_landmarks("session", fence, 1, Some("parent"))
        .await
        .unwrap();
    assert_eq!(
        (parent[0].sequence, parent[0].last_sequence),
        (first * 256, last * 256)
    );
    assert!(
        log.navigation_landmarks("session", fence, 1, Some("missing"))
            .await
            .unwrap()
            .is_empty()
    );
    open(&log, "resumed-parent", "parent", "continued").await;
    let last = end(&log, "resumed-parent", "parent").await;
    prepare(&log, last).await;
    let sampled = log
        .navigation_landmarks(
            "session",
            maka_presentation::watermark(last).unwrap(),
            64,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        sampled.iter().filter(|row| row.turn_id == "parent").count(),
        1
    );
    log.close().await.unwrap();
}

#[tokio::test]
async fn navigation_anchors_fixed_fence_byte_pages_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    assert!(matches!(
        log.navigation_fence("session").await,
        Err(StoreError::SessionNotFound)
    ));
    log.create_session("session", "fingerprint", &json!({}), 1)
        .await
        .unwrap();
    assert_eq!(log.navigation_fence("session").await.unwrap(), None);
    let first = open(
        &log,
        "first",
        "shared",
        &format!("\u{feff}  {}  ", "😀".repeat(100)),
    )
    .await;
    assert_eq!(
        log.navigation_fence("session").await.unwrap(),
        Some(maka_presentation::watermark(first).unwrap())
    );
    end(&log, "first", "shared").await;
    let second = open(&log, "middle", "middle", "middle").await;
    let ending = end(&log, "middle", "middle").await;
    prepare(&log, ending).await;
    let fence = log.navigation_fence("session").await.unwrap().unwrap();
    let initial = pages(&log, fence, 1).await;
    assert_eq!(initial.len(), 2);
    assert_eq!(initial[0].first_sequence, first * 256);
    assert_eq!(initial[1].first_sequence, second * 256);
    assert!(
        initial
            .iter()
            .all(|c| c.latest_state.as_ref().unwrap().message.status == TurnStatus::Completed)
    );
    assert_eq!(
        initial[0].user_prompt_preview.as_deref(),
        Some("😀".repeat(64).as_str())
    );
    let landmarks = log
        .navigation_landmarks("session", fence, 64, None)
        .await
        .unwrap();
    assert_eq!(landmarks.len(), 2);
    assert_eq!(landmarks[0].label, "😀".repeat(24));
    assert_eq!(landmarks[0].sequence, first * 256);
    assert_eq!(
        log.navigation_landmarks("session", fence, 1, None)
            .await
            .unwrap()[0]
            .turn_id,
        "middle"
    );
    let mid = log
        .navigation_turns("session", fence, first * 256 + 1, 1)
        .await
        .unwrap();
    assert_eq!(mid.contributions[0].user_prompt_preview, None);
    assert!(mid.contributions[0].latest_state.is_some());

    let active = open(&log, "active", "active", "not settled").await;
    prepare(&log, active).await;
    let active_fence = maka_presentation::watermark(active).unwrap();
    assert_eq!(
        log.navigation_fence("session").await.unwrap(),
        Some(active_fence)
    );
    let active_landmark = log
        .navigation_landmarks("session", active_fence, 1, Some("active"))
        .await
        .unwrap();
    assert_eq!(active_landmark.len(), 1);
    assert_eq!(active_landmark[0].sequence, active * 256);
    assert_eq!(active_landmark[0].last_sequence, active * 256 + 1);
    let active_turn = pages(&log, active_fence, 128)
        .await
        .into_iter()
        .find(|turn| turn.turn_id == "active")
        .unwrap();
    assert_eq!(
        active_turn.latest_state.unwrap().message.status,
        maka_presentation::navigation::TurnStatus::Running
    );
    assert_eq!(pages(&log, fence, 1).await, initial);
    end(&log, "active", "active").await;
    open(&log, "successor", "shared", "successor").await;
    end(&log, "successor", "shared").await;
    // Deliberately adversarial JSON escaping: byte length is not encoded length.
    for n in 0..128 {
        let run = format!("escaped-{n}");
        open(&log, &run, &run, &"\u{1}".repeat(256)).await;
        end(&log, &run, &run).await;
    }
    let current = log.navigation_fence("session").await.unwrap().unwrap();
    prepare(&log, current / 256).await;
    assert_eq!(
        pages(&log, fence, 1).await,
        initial,
        "later settlement must not alter the old fence"
    );
    let all = pages(&log, current, 128).await;
    assert!(all.iter().any(|c| c.first_sequence == active * 256));
    assert_eq!(all.iter().filter(|c| c.turn_id == "shared").count(), 1);
    assert_eq!(
        all.iter()
            .filter(|c| c.turn_id.starts_with("escaped-") && c.user_prompt_preview.is_some())
            .count(),
        128
    );
    let first_page = log
        .navigation_turns("session", current, 0, 128)
        .await
        .unwrap();
    assert!(first_page.next_position.is_some());
    // Every landmark is an actual persisted user row, not an opening/header guess.
    let db = rusqlite::Connection::open(&path).unwrap();
    for landmark in log
        .navigation_landmarks("session", current, 64, None)
        .await
        .unwrap()
    {
        let payload: Vec<u8> = db
            .query_row(
                "SELECT payload FROM transcript_rows WHERE sequence=? AND session_id='session'",
                [landmark.sequence as i64],
                |row| row.get(0),
            )
            .unwrap();
        let row: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(row["type"], "user");
        assert_eq!(row["turnId"], landmark.turn_id);
    }
    drop(db);
    log.close().await.unwrap();
    let reopened = EventLog::open(&path).await.unwrap();
    assert_eq!(pages(&reopened, fence, 1).await, initial);
    assert_eq!(pages(&reopened, current, 128).await, all);
    reopened.close().await.unwrap();
}
