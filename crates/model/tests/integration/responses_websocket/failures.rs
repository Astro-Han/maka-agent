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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_and_bad_frames_do_not_replay_or_poison_other_lanes() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let (dispatched, received) = tokio::sync::oneshot::channel();
        let (retry_started, retry_received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            reject_upgrade(&listener).await;
            let (mut retry, _) = listener.accept().await.unwrap();
            retry_started.send(()).unwrap();
            let mut pending = [0; 4096];
            while retry.read(&mut pending).await.unwrap() != 0 {}
            // Cancellation of a retry must release its socket, not defer this
            // route or allow a later timer to reconnect behind the caller.
            let mut stalled = accept(&listener).await;
            wire(&mut stalled, "cancel").await;
            dispatched.send(()).unwrap();
            let mut healthy = accept(&listener).await;
            wire(&mut healthy, "healthy").await;
            for event in events("resp_ok", "OK") {
                healthy
                    .send(Message::Text(
                        serde_json::to_string_pretty(&event).unwrap().into(),
                    ))
                    .await
                    .unwrap();
            }
            assert!(
                stalled
                    .next()
                    .await
                    .is_none_or(|frame| frame.is_err() || matches!(frame, Ok(Message::Close(_))))
            );
            let mut incomplete = accept(&listener).await;
            wire(&mut incomplete, "incomplete").await;
            incomplete
                .send(Message::Text(
                    json!({"type":"response.incomplete",
                "response":{"incomplete_details":{"reason":"content_filter"}}})
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let _ = incomplete.next().await;
            let mut bad = Vec::new();
            for _ in 0..3 {
                let mut socket = accept(&listener).await;
                wire(&mut socket, "bad").await;
                bad.push(socket);
            }
            for (mut socket, frame) in bad.into_iter().zip([
                Message::Binary(vec![1, 2].into()),
                Message::Text("{bad".into()),
                Message::Text("x".repeat(8 * 1024 * 1024 + 1).into()),
            ]) {
                let _ = socket.send(frame).await;
                let _ = socket.next().await;
            }
            wire(&mut healthy, "still healthy").await;
            finish(&mut healthy, "resp_healthy_again").await;
            let body = http_ok(&listener).await;
            assert!(
                body["input"]
                    .to_string()
                    .contains("after transport failure")
            );
            assert!(listener.accept().now_or_never().is_none());
        });
        let executor = ModelExecutor::new(4, Duration::from_secs(10)).unwrap();
        let connecting_cancel = CancellationToken::new();
        let connecting = executor
            .stream_in_lane(
                request(&base, "cancel during retry"),
                connecting_cancel.clone(),
                Some(ResponsesLane::default()),
            )
            .await
            .unwrap();
        retry_received.await.unwrap();
        connecting_cancel.cancel();
        connecting.cancel_and_wait().await;
        let healthy_lane = ResponsesLane::default();
        let cancel = CancellationToken::new();
        let mut stalled = executor
            .stream_in_lane(
                request(&base, "cancel"),
                cancel.clone(),
                Some(ResponsesLane::default()),
            )
            .await
            .unwrap();
        received.await.unwrap();
        generate(&executor, &healthy_lane, request(&base, "healthy")).await;
        cancel.cancel();
        while let Some(event) = stalled.next().await {
            if event.is_err() {
                break;
            }
        }
        stalled.cancel_and_wait().await;
        let incomplete = executor
            .stream_in_lane(
                request(&base, "incomplete"),
                CancellationToken::new(),
                Some(ResponsesLane::default()),
            )
            .await
            .unwrap();
        assert_failed(incomplete).await;
        let mut streams = Vec::new();
        for _ in 0..3 {
            let stream = executor
                .stream_in_lane(
                    request(&base, "bad"),
                    CancellationToken::new(),
                    Some(ResponsesLane::default()),
                )
                .await
                .unwrap();
            streams.push(stream);
        }
        for stream in streams {
            assert_failed(stream).await;
        }
        generate(&executor, &healthy_lane, request(&base, "still healthy")).await;
        generate(
            &executor,
            &ResponsesLane::default(),
            request(&base, "after transport failure"),
        )
        .await;
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_upgrade_falls_back_before_dispatch_and_stays_http() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut attempts = Vec::new();
            for expected in ["GET"; 6].into_iter().chain(["POST"; 3]) {
                let (mut socket, _) = listener.accept().await.unwrap();
                if expected == "GET" { attempts.push(std::time::Instant::now()); }
                let mut bytes = Vec::new();
                let boundary = loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(index) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") { break index + 4; }
                };
                let head = String::from_utf8_lossy(&bytes[..boundary]);
                assert!(head.starts_with(&format!("{expected} http://models.maka.invalid/v1/responses ")));
                assert!(head.to_ascii_lowercase().contains("\r\nproxy-authorization: basic dxnlcjpzzwnyzxq=\r\n"));
                if expected == "GET" {
                    socket.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                } else {
                    let length: usize = head.lines().find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
                    }).unwrap();
                    while bytes.len() < boundary + length {
                        let mut buffer = [0; 4096];
                        let count = socket.read(&mut buffer).await.unwrap();
                        assert_ne!(count, 0);
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    let body: Value = serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap();
                    assert_eq!(body["stream"], true);
                    assert!(body.get("type").is_none());
                    let response: String = events("resp_http", "OK").iter().map(|event| format!("data: {event}\n\n")).collect();
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
                }
            }
            for (index, pair) in attempts.windows(2).enumerate() {
                assert!(pair[1].duration_since(pair[0]) >= Duration::from_millis(100 << index),
                    "retry {index} must wait its exponential interval");
            }
        });
        let executor = ModelExecutor::new(1, Duration::from_secs(10)).unwrap();
        let lane = ResponsesLane::default();
        generate(&executor, &lane, proxied_request(proxy_port, "first")).await;
        generate(&executor, &lane, proxied_request(proxy_port, "second")).await;
        drop(lane);
        generate(&executor, &ResponsesLane::default(), proxied_request(proxy_port, "new Turn")).await;
        server.await.unwrap();
    }).await.unwrap();
}

async fn assert_failed(mut stream: maka_model::ModelStream) {
    let mut failed = false;
    while let Some(event) = stream.next().await {
        if event.is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
    stream.cancel_and_wait().await;
}

async fn http_ok(listener: &TcpListener) -> Value {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut head = Vec::new();
    loop {
        head.push(socket.read_u8().await.unwrap());
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("POST /v1/responses "));
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    let mut bytes = vec![0; length];
    socket.read_exact(&mut bytes).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["stream"], true);
    assert!(body.get("previous_response_id").is_none());
    let response: String = events("resp_http", "OK")
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
    body
}
