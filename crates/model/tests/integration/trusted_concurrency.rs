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

use super::provider_stream::{read_request, request};
use maka_js_runtime::trusted::TrustedRuntime;
use maka_model::{ModelEvent, ModelExecutor, ProviderKind, StepBuilder};
use serde_json::json;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_sdk_requests_progress_through_backpressure_and_cancellation() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let runtime = TrustedRuntime::default();
        let models = ModelExecutor::with_runtime(runtime.clone(), 4, Duration::from_secs(15)).unwrap();

        let waiting = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let waiting_base = format!("http://{}/v1", waiting.local_addr().unwrap());
        let (received, ready) = oneshot::channel();
        let pending_http = tokio::spawn(async move {
            let (mut socket, _) = waiting.accept().await.unwrap();
            read_request(&mut socket).await;
            received.send(()).unwrap();
            assert_http_closed(&mut socket).await;
        });
        let held = models.stream(request(ProviderKind::OpenaiChat, waiting_base), CancellationToken::new()).await.unwrap();
        ready.await.unwrap();

        let flooding = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let flooding_base = format!("http://{}/v1", flooding.local_addr().unwrap());
        let flood_http = tokio::spawn(async move {
            let (mut socket, _) = flooding.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
            let part = json!({"id":"slow","object":"chat.completion.chunk","created":1,"model":"test-model",
                "choices":[{"index":0,"delta":{"content":"x".repeat(4096)},"finish_reason":null}]});
            let data = format!("data: {part}\n\n").repeat(256);
            socket.write_all(format!("{:x}\r\n{data}\r\n", data.len()).as_bytes()).await.unwrap();
            assert_http_closed(&mut socket).await;
        });
        let mut slow = models.stream(request(ProviderKind::OpenaiChat, flooding_base), CancellationToken::new()).await.unwrap();
        loop {
            if matches!(slow.next().await.unwrap().unwrap(), ModelEvent::PartDelta { .. }) { break; }
        }
        // Leave the stream alive and undrained. More events than its bounded
        // output queue can hold must not occupy the shared worker indefinitely.
        let serving = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", serving.local_addr().unwrap());
        let healthy_http = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut socket, _) = serving.accept().await.unwrap();
                read_request(&mut socket).await;
                let part = json!({"id":"healthy","object":"chat.completion.chunk","created":1,"model":"test-model",
                    "choices":[{"index":0,"delta":{"content":"alive"},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}});
                let body = format!("data: {part}\n\ndata: [DONE]\n\n");
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        async fn complete(models: &ModelExecutor, base: &str) {
            let mut stream = models.stream(request(ProviderKind::OpenaiChat, base.into()), CancellationToken::new()).await.unwrap();
            let mut step = StepBuilder::default();
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                if let ModelEvent::PartDelta { text: delta, .. } = &event { text.push_str(delta); }
                step.push(event).unwrap();
            }
            assert_eq!(text, "alive");
            step.finish().unwrap();
            stream.cancel_and_wait().await;
        }
        complete(&models, &base).await;

        held.cancel_and_wait().await;
        pending_http.await.unwrap();
        // An abandoned model consumer also cancels and settles its SDK request.
        let abandoned = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let abandoned_base = format!("http://{}/v1", abandoned.local_addr().unwrap());
        let (arrived, ready) = oneshot::channel();
        let abandoned_http = tokio::spawn(async move {
            let (mut socket, _) = abandoned.accept().await.unwrap();
            read_request(&mut socket).await;
            arrived.send(()).unwrap();
            assert_http_closed(&mut socket).await;
        });
        let abandoned_models = models.clone();
        let abandoned_call = tokio::spawn(async move {
            let _stream = abandoned_models.stream(request(ProviderKind::OpenaiChat, abandoned_base), CancellationToken::new()).await.unwrap();
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        abandoned_call.abort();
        assert!(abandoned_call.await.unwrap_err().is_cancelled());
        abandoned_http.await.unwrap();
        complete(&models, &base).await;

        slow.cancel_and_wait().await;
        flood_http.await.unwrap();
        complete(&models, &base).await;
        healthy_http.await.unwrap();
    }).await.expect("one trusted V8 must make progress across independent lifetimes");
}

async fn assert_http_closed(socket: &mut TcpStream) {
    match socket.read(&mut [0]).await {
        Ok(0) => {}
        // Cancellation may reset a socket with unread response bytes. Winsock
        // also reports this as ConnectionAborted, rather than an orderly EOF.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ) => {}
        result => panic!("cancelled HTTP connection did not close: {result:?}"),
    }
}
