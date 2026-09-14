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

use crate::support::context as support;
use maka_event_log::EventLog;
use maka_runtime::{
    context::CompactOutcome,
    event::{Fact, InvocationOutcome},
};
use std::sync::Arc;
use support::*;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_next_main_and_reopen_use_durable_baseline_with_complete_tail() {
    let input = |base: &str, id: &str, compact| {
        let mut input = support::input(base, id, compact);
        input.configuration.system_prompt = Some(maka_runtime::execution::SystemPrompt {
            text: "MAIN_SYSTEM_PROMPT_EVIDENCE".into(),
            policy_revision: 3,
        });
        input
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for text in [
            "old-answer-private",
            SUMMARY,
            "tail-answer",
            "reopened-answer",
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut socket).await);
            respond(&mut socket, text, "stop").await;
        }
        requests
    });
    let log = Arc::new(EventLog::open(&path).await.unwrap());
    let worker = engine(log.clone());
    worker
        .run(input(&base, "old", false), CancellationToken::new())
        .await
        .unwrap();
    worker
        .run(input(&base, "compact", true), CancellationToken::new())
        .await
        .unwrap();
    let source = log
        .read_model_context("session", None, 100, 128 * 1024)
        .await
        .unwrap();
    let checkpoint = source.baseline.unwrap().event_id;
    worker
        .run(input(&base, "tail", false), CancellationToken::new())
        .await
        .unwrap();
    worker.drain().await;
    drop(worker);
    Arc::try_unwrap(log).ok().unwrap().close().await.unwrap();
    let log = Arc::new(EventLog::open(&path).await.unwrap());
    let worker = engine(log.clone());
    worker
        .run(input(&base, "reopen", false), CancellationToken::new())
        .await
        .unwrap();
    worker.drain().await;
    let requests = server.await.unwrap();
    for index in [0, 2, 3] {
        assert_eq!(requests[index]["messages"][0]["role"], "system");
        assert_eq!(
            requests[index]["messages"][0]["content"],
            "MAIN_SYSTEM_PROMPT_EVIDENCE"
        );
    }
    assert!(
        !requests[1]
            .to_string()
            .contains("MAIN_SYSTEM_PROMPT_EVIDENCE")
    );
    assert!(
        requests[1]["messages"]
            .to_string()
            .contains("old-answer-private")
    );
    assert_eq!(requests[1]["messages"][0]["role"], "user");
    assert!(
        requests[1]["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("Now write the structured summary")
    );
    assert!(requests[1]["tools"].as_array().is_none_or(Vec::is_empty));
    assert_eq!(requests[1]["max_tokens"], 8000);
    for request in &requests[2..] {
        assert!(
            request["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains(SUMMARY)
        );
        assert!(
            !request["messages"]
                .to_string()
                .contains("old-answer-private")
        );
        assert!(
            !request["messages"]
                .to_string()
                .contains("Now write the structured summary")
        );
    }
    assert!(requests[3]["messages"].to_string().contains("tail-answer"));
    let prefix = log.prefix(100, 256 * 1024).await.unwrap();
    for event in prefix.events.iter().filter(|event| {
        matches!(
            event.event.invocation.invocation_id.as_str(),
            "invocation-tail" | "invocation-reopen"
        )
    }) {
        if let Fact::ModelRequested {
            checkpoint_event_id,
            ..
        } = &event.event.fact
        {
            assert_eq!(checkpoint_event_id.as_ref(), Some(&checkpoint));
        }
    }
    let legacy = maka_agent::project_model_history(&prefix, "session").unwrap();
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("summary-marker"),
        "summary attempts never become ordinary history"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_summary_repairs_once_then_keeps_previous_context() {
    let directory = tempfile::tempdir().unwrap();
    let log = Arc::new(
        EventLog::open(&directory.path().join("events.sqlite"))
            .await
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for text in [
            "old-answer",
            "invalid-summary",
            "still-invalid",
            "next-answer",
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut socket).await);
            respond(&mut socket, text, "stop").await;
        }
        requests
    });
    let worker = engine(log.clone());
    for (id, compact) in [("old", false), ("bad", true), ("next", false)] {
        worker
            .run(input(&base, id, compact), CancellationToken::new())
            .await
            .unwrap();
    }
    let prefix = log.prefix(100, 256 * 1024).await.unwrap();
    assert!(
        !prefix
            .events
            .iter()
            .any(|event| matches!(event.event.fact, Fact::ContextCheckpointRecorded { .. }))
    );
    assert!(prefix.events.iter().any(|event|matches!(&event.event.fact,Fact::InvocationEnded {outcome:InvocationOutcome::ContextCompactFinished {outcome:CompactOutcome::Failed {reason}}} if reason=="malformed_summary_missing_section")));
    let requests = server.await.unwrap();
    assert!(
        requests[2]["messages"]
            .to_string()
            .contains("A prior attempt was rejected")
    );
    assert!(requests[3]["messages"].to_string().contains("old-answer"));
    assert!(
        !requests[3]["messages"]
            .to_string()
            .contains("invalid-summary")
    );
    worker.drain().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn length_retries_once_for_compact_but_never_accepts_truncated_main() {
    let directory = tempfile::tempdir().unwrap();
    let log = Arc::new(
        EventLog::open(&directory.path().join("events.sqlite"))
            .await
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (text, reason) in [
            ("old", "stop"),
            ("cut", "length"),
            (SUMMARY, "stop"),
            ("truncated-main", "length"),
            ("after-failure", "stop"),
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut socket).await);
            respond(&mut socket, text, reason).await;
        }
        requests
    });
    let worker = engine(log.clone());
    worker
        .run(input(&base, "old", false), CancellationToken::new())
        .await
        .unwrap();
    worker
        .run(input(&base, "shorten", true), CancellationToken::new())
        .await
        .unwrap();
    assert!(
        worker
            .run(input(&base, "truncated", false), CancellationToken::new())
            .await
            .is_err()
    );
    worker
        .run(
            input(&base, "after-failure", false),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert!(
        !requests[4]["messages"]
            .to_string()
            .contains("truncated-main"),
        "failed response remains display evidence, never model replay"
    );
    assert!(
        requests[2]["messages"]
            .to_string()
            .contains("well under half the length")
    );
    let prefix = log.prefix(100, 256 * 1024).await.unwrap();
    assert!(
        prefix
            .events
            .iter()
            .any(|event| matches!(event.event.fact, Fact::ContextCheckpointRecorded { .. }))
    );
    let truncated: Vec<_> = prefix
        .events
        .iter()
        .filter(|event| event.event.invocation.invocation_id == "invocation-truncated")
        .collect();
    assert!(
        truncated
            .iter()
            .any(|event| matches!(event.event.fact, Fact::ModelInterrupted { .. }))
    );
    assert!(
        !truncated
            .iter()
            .any(|event| matches!(event.event.fact, Fact::ModelCompleted { .. }))
    );
    worker.drain().await;
}
