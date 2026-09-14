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

use maka_model::connection::{ConnectionClient, DiscoveryKind, DiscoveryRequest};
use maka_runtime::configuration::ConnectionEffectFailureClass as Failure;
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
mod subscription;

async fn fixture(response: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let byte = socket.read_u8().await.unwrap();
            request.push(byte);
            assert!(request.len() < 8192);
        }
        // Oversized/invalid responses may be rejected before the write completes.
        let _ = socket.write_all(&response).await;
        String::from_utf8(request).unwrap()
    });
    (url, task)
}

#[tokio::test]
async fn native_wires_add_private_headers_but_reject_generated_header_conflicts() {
    let discovery = ConnectionClient::new().unwrap();
    for kind in [DiscoveryKind::Openai, DiscoveryKind::Anthropic] {
        let response = if matches!(kind, DiscoveryKind::Anthropic) {
            // gzip of the same JSON used by the uncompressed OpenAI response.
            let mut response =
                b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nConnection: close\r\n\r\n".to_vec();
            response.extend_from_slice(&[
                31, 139, 8, 0, 0, 0, 0, 0, 0, 19, 171, 86, 74, 73, 44, 73, 84, 178, 138, 174, 86,
                202, 76, 81, 178, 82, 82, 72, 206, 200, 47, 78, 205, 83, 80, 210, 81, 74, 206, 207,
                43, 73, 173, 40, 137, 207, 73, 205, 75, 47, 201, 80, 178, 178, 48, 180, 52, 170,
                141, 173, 5, 0, 183, 24, 245, 41, 50, 0, 0, 0,
            ]);
            response
        } else {
            b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"data\":[{\"id\":\" chosen \",\"context_length\":8192}]}".to_vec()
        };
        let (base, server) = fixture(response).await;
        let headers = BTreeMap::from([
            ("X-Route".into(), "private-route".into()),
            ("anthropic-version".into(), "2023-06-01".into()),
        ]);
        let result = discovery
            .fetch(DiscoveryRequest {
                kind,
                base_url: &format!("{base}/v1/"),
                credential: "fixture-key",
                headers: &headers,
            })
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!([{"id":"chosen","contextWindow":8192}])
        );
        let request = server.await.unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /v1/models http/1.1\r\n"));
        assert!(request.contains("\r\nx-route: private-route\r\n"));
        if matches!(kind, DiscoveryKind::Anthropic) {
            assert!(request.contains("\r\nx-api-key: fixture-key\r\n"));
            assert!(request.contains("\r\nanthropic-version: 2023-06-01\r\n"));
        } else {
            assert!(request.contains("\r\nauthorization: bearer fixture-key\r\n"));
            assert!(request.contains("\r\ncontent-type: application/json\r\n"));
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let headers = BTreeMap::from([("anthropic-version".into(), "conflicting".into())]);
    assert_eq!(
        discovery
            .fetch(DiscoveryRequest {
                kind: DiscoveryKind::Anthropic,
                base_url: &base,
                credential: "fixture",
                headers: &headers,
            })
            .await,
        Err(Failure::Network)
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn status_shape_and_streamed_body_limits_fail_without_accepting_partial_inventory() {
    let discovery = ConnectionClient::new().unwrap();
    let headers = BTreeMap::new();
    let mut oversized = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    oversized.resize(oversized.len() + 4 * 1024 * 1024 + 1, b' ');
    for (response, expected) in [
        (
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec(),
            Failure::Auth,
        ),
        (
            b"HTTP/1.1 429 Busy\r\nContent-Length: 0\r\n\r\n".to_vec(),
            Failure::ProviderUnavailable,
        ),
        (
            b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"data\":[]}".to_vec(),
            Failure::InvalidResponse,
        ),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 4194305\r\n\r\n".to_vec(),
            Failure::InvalidResponse,
        ),
        (oversized, Failure::InvalidResponse),
    ] {
        let (base, server) = fixture(response).await;
        assert_eq!(
            discovery
                .fetch(DiscoveryRequest {
                    kind: DiscoveryKind::Openai,
                    base_url: &base,
                    credential: "fixture",
                    headers: &headers,
                })
                .await,
            Err(expected)
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn total_deadline_remains_active_after_headers_and_releases_the_stalled_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
            .await
            .unwrap();
        // EOF proves the timed-out response body did not leave an open request.
        assert_eq!(socket.read(&mut [0; 1]).await.unwrap(), 0);
    });
    let discovery = ConnectionClient::new().unwrap();
    let headers = BTreeMap::new();
    let request = discovery.fetch(DiscoveryRequest {
        kind: DiscoveryKind::Openai,
        base_url: &base,
        credential: "fixture",
        headers: &headers,
    });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(12), request)
            .await
            .unwrap(),
        Err(Failure::Timeout)
    );
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}
