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
use maka_plugins::session::history::{Page, Role};
use std::collections::BTreeMap;

#[tokio::test]
async fn history_pages_preserve_utf8_nul_suffixes_archive_and_frozen_fences() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("log.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("a", "create", &serde_json::json!({}), 1)
        .await
        .unwrap();
    let text = format!("a{}\0suffix-needle", "🦀".repeat(10_000));
    let (_, old) = partial(&log, "a", &text).await;
    interrupt(&log, "a").await;
    log.append(&event("a", request("history"))).await.unwrap();
    let call = maka_runtime::model::ModelToolCall {
        id: "read".into(),
        name: "Read".into(),
        input: serde_json::json!({"path": "source.rs", "intent": "Inspect source"}),
        provider_options: None,
        provider_executed: false,
    };
    log.append(&event(
        "a",
        Fact::ModelObserved {
            step_id: "step-history".into(),
            event: ModelEvent::ToolCall(call.clone()),
        },
    ))
    .await
    .unwrap();
    log.append(&event(
        "a",
        Fact::ModelCompleted {
            step_id: "step-history".into(),
            output: ModelStep {
                parts: vec![ModelPart::ToolCall { call }],
                finish_reason: maka_runtime::model::ModelFinishReason::ToolCalls,
                usage: ModelUsage::default(),
                provider_options: None,
                response_id: None,
                model: None,
                timestamp: None,
            },
        },
    ))
    .await
    .unwrap();
    log.append(&event(
        "a",
        Fact::ToolDispatched {
            operation_id: "step-history:read".into(),
            call: maka_runtime::tool_call::ToolCallIdentity::provider(
                "step-history".into(),
                "read".into(),
            ),
            name: "Read".into(),
            input: serde_json::json!({"path": "source.rs", "intent": "Inspect source"}),
        },
    ))
    .await
    .unwrap();
    let template = event(
        "a",
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    );
    let (result, _) = maka_runtime::event::EventWrite::tool_success(
        "read-result".into(),
        template.event().recorded_at,
        template.event().invocation.clone(),
        "step-history:read".into(),
        serde_json::json!({"content": text, "nested": ["second\nline", 42, false]}).into(),
    )
    .unwrap();
    log.append(&result).await.unwrap();
    let through = end(
        &log,
        "a",
        InvocationOutcome::Cancelled {
            source: "user".into(),
        },
    )
    .await;
    assert!(log.prepare_transcript("a", through, 32).await.unwrap());
    log.set_session_archived::<Value>("a", true, 10)
        .await
        .unwrap();
    let archived = log
        .scoped_sessions::<Value>(
            maka_event_log::sessions::CatalogScope::Profile,
            None,
            None,
            true,
        )
        .await
        .unwrap();
    assert!(archived.sessions[0].archived);
    let mut cursor = None;
    let mut messages: BTreeMap<String, (Role, String)> = BTreeMap::new();
    let mut pages = 0;
    loop {
        let page = log.history_text("a", through, cursor).await.unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < 256 * 1024);
        let Page::Ready { chunks, next, .. } = page else {
            panic!("prepared")
        };
        for chunk in chunks {
            let entry = messages
                .entry(chunk.message_id)
                .or_insert((chunk.role, String::new()));
            assert_eq!(entry.1.len() as u64, chunk.offset);
            entry.1.push_str(&chunk.text);
            assert!(entry.1.len() as u64 <= chunk.total_bytes);
        }
        pages += 1;
        assert!(pages < 20, "continuation must advance");
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    assert!(pages >= 3, "large message must be split, not clipped");
    assert_eq!(
        messages
            .values()
            .find(|(role, _)| *role == Role::Assistant)
            .unwrap()
            .1,
        text
    );
    assert_eq!(
        messages
            .values()
            .find(|(role, _)| *role == Role::ToolResult)
            .unwrap()
            .1,
        format!("{text}\nsecond\nline"),
        "JSON string values stay multiline; property names and non-text leaves are not searchable"
    );
    let Page::Ready { chunks, next, .. } = log.history_text("a", old, None).await.unwrap() else {
        panic!("prepared")
    };
    assert!(next.is_none());
    assert!(
        chunks.iter().all(|chunk| chunk.role == Role::User),
        "old fence excludes unfinished answer"
    );
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    let Page::Ready { chunks, .. } = log.history_text("a", through, None).await.unwrap() else {
        panic!("prepared")
    };
    assert!(!chunks.is_empty(), "restart retains readable archive");
    log.close().await.unwrap();
}
