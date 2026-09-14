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

use maka_model::{ProviderConfig, ProviderKind, connection::ConnectionClient};
use maka_runtime::configuration::ConnectionEffectFailureClass as Failure;
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn provider(kind: ProviderKind, base_url: String) -> ProviderConfig {
    ProviderConfig {
        kind,
        model: "Test-Model".into(),
        base_url,
        auth: maka_model::ProviderAuth::ApiKey("Fixture-Key".into()),
        headers: BTreeMap::new(),
        network: Default::default(),
        body_overlay: None,
    }
}

async fn fixture(
    response: Vec<u8>,
    stall: bool,
) -> (String, tokio::task::JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(socket.read_u8().await.unwrap());
            assert!(headers.len() < 8192);
        }
        let headers = String::from_utf8(headers).unwrap();
        let length: usize = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .unwrap()
            .1
            .trim()
            .parse()
            .unwrap();
        assert!(length < 8192);
        let mut body = vec![0; length];
        socket.read_exact(&mut body).await.unwrap();
        let body = serde_json::from_slice(&body).unwrap();
        // A bounded reader may reject the response before this write completes.
        let _ = socket.write_all(&response).await;
        if stall {
            // Incomplete bodies must be cancelled, releasing the connection.
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), socket.read(&mut [0; 1]))
                    .await
                    .expect("probe left its response connection open")
                    .unwrap(),
                0
            );
        }
        (headers, body)
    });
    (url, task)
}

#[tokio::test]
async fn native_posts_preserve_wire_contract_and_accept_additional_customization() {
    let client = ConnectionClient::new().unwrap();
    for (kind, suffix, path) in [
        (ProviderKind::OpenaiChat, "/v1/", "/v1/chat/completions"),
        (
            ProviderKind::OpenaiCompatible {
                name: "gateway".into(),
            },
            "/v1/",
            "/v1/chat/completions",
        ),
        (ProviderKind::OpenaiResponses, "/v1/", "/v1/responses"),
        (
            ProviderKind::OpenaiResponses,
            "/v1/responses/",
            "/v1/responses",
        ),
        (ProviderKind::Anthropic, "/v1/", "/v1/messages"),
    ] {
        let (base, server) = fixture(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nnot-json".to_vec(),
            false,
        )
        .await;
        let mut config = provider(kind.clone(), format!("{base}{suffix}"));
        config.headers = BTreeMap::from([
            ("X-Route".into(), "private-route".into()),
            ("Content-Type".into(), "application/json".into()),
        ]);
        let overlay = json!({"temperature":0.25,"metadata":{"route":"private"}});
        config.body_overlay = Some(overlay.as_object().unwrap().clone());
        client.test(&config).await.unwrap();
        let (headers, body) = server.await.unwrap();
        assert!(headers.starts_with(&format!("POST {path} HTTP/1.1\r\n")));
        assert!(headers.contains("\r\nx-route: private-route\r\n"));
        assert!(headers.contains("\r\ncontent-type: application/json\r\n"));
        if matches!(kind, ProviderKind::Anthropic) {
            assert!(headers.contains("\r\nx-api-key: Fixture-Key\r\n"));
            assert!(headers.contains("\r\nanthropic-version: 2023-06-01\r\n"));
            assert!(!headers.contains("\r\nauthorization:"));
        } else {
            assert!(headers.contains("\r\nauthorization: Bearer Fixture-Key\r\n"));
            assert!(!headers.contains("\r\nx-api-key:"));
        }
        let mut expected = if matches!(kind, ProviderKind::OpenaiResponses) {
            json!({"model":"Test-Model","store":false,"max_output_tokens":16,
                "input":[{"role":"user","content":"Hi"}]})
        } else {
            json!({"model":"Test-Model","max_tokens":16,
                "messages":[{"role":"user","content":"Hi"}]})
        };
        expected["temperature"] = json!(0.25);
        expected["metadata"] = json!({"route":"private"});
        assert_eq!(body, expected);
    }
}

#[tokio::test]
async fn successful_and_rate_limited_probes_cancel_without_waiting_for_bodies() {
    let client = ConnectionClient::new().unwrap();
    for status in [200, 202, 429] {
        let (base, server) = fixture(
            format!("HTTP/1.1 {status} Test\r\nContent-Length: 100000\r\n\r\nnot-json")
                .into_bytes(),
            true,
        )
        .await;
        let config = provider(ProviderKind::OpenaiChat, base);
        let result = tokio::time::timeout(Duration::from_secs(2), client.test(&config))
            .await
            .expect("probe waited for an unnecessary response body");
        if status == 429 {
            let failure = result.unwrap_err();
            assert_eq!(failure.class, Failure::ProviderUnavailable);
            assert_eq!(failure.status_code, Some(429));
        } else {
            result.unwrap();
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn error_body_limit_accepts_16_kib_and_rejects_larger_declared_or_streamed_bodies() {
    let client = ConnectionClient::new().unwrap();
    for (length, declared) in [
        (16 * 1024, false),
        (16 * 1024 + 1, false),
        (16 * 1024 + 1, true),
    ] {
        let mut response = if declared {
            format!("HTTP/1.1 401 Unauthorized\r\nContent-Length: {length}\r\n\r\n").into_bytes()
        } else {
            b"HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n".to_vec()
        };
        if !declared {
            response.resize(response.len() + length, b'x');
        }
        let (base, server) = fixture(response, declared).await;
        let config = provider(ProviderKind::OpenaiChat, base);
        let failure = tokio::time::timeout(Duration::from_secs(2), client.test(&config))
            .await
            .unwrap()
            .unwrap_err();
        if length == 16 * 1024 {
            assert_eq!(failure.class, Failure::Auth);
            assert_eq!(failure.status_code, Some(401));
        } else {
            assert_eq!(failure.class, Failure::InvalidResponse);
            assert_eq!(failure.status_code, None);
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn generated_body_and_header_conflicts_fail_before_network_io() {
    let client = ConnectionClient::new().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    for kind in [
        ProviderKind::OpenaiChat,
        ProviderKind::OpenaiResponses,
        ProviderKind::Anthropic,
    ] {
        let mut config = provider(kind.clone(), base.clone());
        let collisions = if matches!(kind, ProviderKind::OpenaiResponses) {
            vec![
                json!({"model":"Test-Model"}),
                json!({"store":false}),
                json!({"max_output_tokens":16}),
                json!({"input":[]}),
            ]
        } else {
            vec![
                json!({"model":"Test-Model"}),
                json!({"max_tokens":16}),
                json!({"messages":[]}),
            ]
        };
        for overlay in collisions {
            config.body_overlay = Some(overlay.as_object().unwrap().clone());
            let failure = tokio::time::timeout(Duration::from_secs(2), client.test(&config))
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(failure.class, Failure::Network);
            assert_eq!(failure.status_code, None);
        }
        config.body_overlay = None;
        let headers = if matches!(kind, ProviderKind::Anthropic) {
            vec!["X-API-Key", "Anthropic-Version", "Content-Type"]
        } else {
            vec!["Authorization", "Content-Type"]
        };
        for header in headers {
            config.headers = BTreeMap::from([(header.into(), "conflict".into())]);
            let failure = tokio::time::timeout(Duration::from_secs(2), client.test(&config))
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(failure.class, Failure::Network);
            assert_eq!(failure.status_code, None);
        }
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
}
