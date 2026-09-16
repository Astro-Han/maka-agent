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

use super::{provider_compatible::body as read_body, provider_stream::request};
use maka_model::{ModelError, ModelExecutor, ProviderKind};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{io::AsyncWriteExt, net::TcpListener};
use tokio_util::sync::CancellationToken;

async fn failure(kind: ProviderKind, status: u16, content_type: &str, body: String) -> ModelError {
    failure_with_headers(kind, status, content_type, body, &[]).await
}

async fn failure_with_headers(
    kind: ProviderKind,
    status: u16,
    content_type: &str,
    body: String,
    extra_headers: &[(&str, &str)],
) -> ModelError {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let extra_headers: String = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    let headers = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n",
        body.len()
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_body(&mut socket).await;
        socket.write_all(headers.as_bytes()).await.unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
        listener
    });
    let executor = ModelExecutor::new(1, Duration::from_secs(10)).unwrap();
    let mut stream = executor
        .stream(request(kind, base), CancellationToken::new())
        .await
        .unwrap();
    let error = loop {
        match stream.next().await {
            Some(Ok(_)) => {}
            Some(Err(error)) => break error,
            None => panic!("provider failure was lost"),
        }
    };
    stream.cancel_and_wait().await;
    let listener = server.await.unwrap();
    // No retry belongs to this boundary, even for a recoverable rejection.
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    error
}

fn sse(parts: &[Value]) -> String {
    parts
        .iter()
        .map(|part| format!("data: {part}\n\n"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_and_sse_failures_preserve_typed_evidence_across_sdk_families() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for kind in [ProviderKind::OpenaiChat, ProviderKind::OpenaiResponses,
            ProviderKind::OpenaiCompatible { name: "fixture".into() }, ProviderKind::Anthropic] {
            let anthropic = matches!(kind, ProviderKind::Anthropic);
            let error = if anthropic {
                json!({"type":"invalid_request_error","message":"prompt is too long: 200001 tokens > 200000 maximum"})
            } else {
                json!({"type":"invalid_request_error","code":"context_length_exceeded","message":"input rejected"})
            };
            let http = json!({"type":"error","error":error});
            assert!(matches!(failure(kind.clone(),400,"application/json",http.to_string()).await,
                ModelError::ContextOverflow { observed_output: false }));
            let frame = if matches!(kind, ProviderKind::OpenaiResponses) {
                json!({"type":"response.failed","sequence_number":0,"response":{"error":{"code":"context_length_exceeded","message":"input rejected"}}})
            } else { http };
            assert!(matches!(failure(kind,200,"text/event-stream",sse(&[frame])).await,
                ModelError::ContextOverflow { observed_output: false }));
        }
        for body in [json!({"error":{"message":"context length rejected"}}).to_string(), String::new()] {
            assert!(matches!(failure(ProviderKind::OpenaiChat,413,"application/json",body).await,ModelError::Adapter(_)));
        }
        for (status, header, reason) in [
            (503, None, Some(maka_model::ProviderFailureReason::ProviderUnavailable)),
            (503, Some("invalid"), Some(maka_model::ProviderFailureReason::ProviderUnavailable)),
            (429, None, Some(maka_model::ProviderFailureReason::RateLimit)),
            (429, Some("0.01"), Some(maka_model::ProviderFailureReason::RateLimit)),
            (401, Some("0.01"), None),
        ] {
            let headers: Vec<_> = header.map(|value| ("Retry-After", value)).into_iter().collect();
            let result = failure_with_headers(ProviderKind::OpenaiChat, status, "application/json",
                json!({"error":{"message":"temporary fixture"}}).to_string(), &headers).await;
            match (reason, result) {
                (Some(reason), ModelError::Provider(failure)) => {
                    assert_eq!(failure.reason(), reason);
                    assert!(failure.replay_safe());
                    assert_eq!(failure.retry_after(), header.filter(|h| *h == "0.01").map(|_| Duration::from_millis(10)));
                }
                (None, ModelError::Adapter(_)) => {}
                (_, result) => panic!("unexpected failure classification: {result}"),
            }
        }
        for kind in [ProviderKind::OpenaiChat, ProviderKind::OpenaiCompatible { name: "fixture".into() }] {
            let chunk = json!({"id":"reply","object":"chat.completion.chunk","created":1,"model":"test-model",
                "choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]});
            let error = failure(kind, 200, "text/event-stream", sse(&[chunk])).await;
            assert!(matches!(error, ModelError::Provider(failure)
                if failure.reason() == maka_model::ProviderFailureReason::StreamTruncated && failure.replay_safe()));
        }
    }).await.expect("bounded SDK error classification");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_and_unfinished_tool_input_make_overflow_non_replayable() {
    tokio::time::timeout(Duration::from_secs(20), async {
        for (kind, delta) in [
            (ProviderKind::OpenaiChat,json!({"role":"assistant","content":"already visible"})),
            (ProviderKind::OpenaiCompatible{name:"fixture".into()},json!({"tool_calls":[{"index":0,"id":"call","type":"function","function":{"name":"echo","arguments":"{"}}]})),
        ] {
            let chunk = json!({"id":"reply","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":delta,"finish_reason":null}]});
            let error = json!({"error":{"type":"invalid_request_error","code":"context_length_exceeded","message":"input rejected"}});
            assert!(matches!(failure(kind,200,"text/event-stream",sse(&[chunk,error])).await,
                ModelError::ContextOverflow { observed_output: true }));
        }
    }).await.expect("partial output must retain its failure classification");
}
