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
use maka_protocol::configuration::ConnectionCatalogQueryInput;

#[test]
fn connection_setup_verifies_without_writes_masks_keys_and_creates_first_chat() {
    let directory = tempfile::tempdir().unwrap();
    let mut host = super::super::candidate::CandidateFixture::new(directory.path().join("root"));
    host.child = Some(
        Command::new(env!("CARGO_BIN_EXE_maka"))
            .args(["host", "serve", "--root"])
            .arg(&host.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    host.wait_for_registration();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (client, url, provider) = runtime.block_on(async {
        let client = support::client(&host.root).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let provider = tokio::spawn(serve(listener));
        (client, url, provider)
    });
    let mut tui = Pty::spawn_at(
        &["--root", host.root.to_str().unwrap()],
        Some(directory.path()),
    );
    tui.wait_for("Workspace");
    tui.click_text("⛭ Settings");
    tui.wait_for("Model connections");
    tui.click_text("Model connections");
    tui.wait_for("No model connections yet.");
    tui.click_text("⊕");
    tui.wait_for("API key");
    tui.wait_for("Verify"); // The modal may arrive across multiple PTY reads.
    tui.send(b"\t");
    tui.send(b"New connection");
    tui.send(b"\t");
    tui.send(format!("\x1b[200~{url}\x1b[201~").as_bytes());
    tui.send(b"\twrong-onboarding-secret");
    tui.click_last_text("Verify");
    tui.wait_for("The service rejected this key.");
    assert!(!String::from_utf8_lossy(&tui.output).contains("wrong-onboarding-secret"));
    tui.click_text("API key");
    tui.send(b"\x01good-onboarding-secret");
    tui.click_last_text("Verify");
    tui.wait_for("Choose models");
    tui.wait_for("Save connection");
    assert_eq!(
        runtime
            .block_on(client.connection_catalog(ConnectionCatalogQueryInput::Start))
            .unwrap()["connectionCount"],
        0,
        "verification cannot leave a half-configured connection or credential"
    );
    tui.click_text("fixture-model");
    tui.wait_for("[x] fixture-model");
    tui.click_last_text("Save connection");
    tui.wait_until(|s| {
        !s.contains("API key") && !s.contains("Choose models") && s.contains("New connection")
    });
    runtime.block_on(async {
        let catalog = client
            .connection_catalog(ConnectionCatalogQueryInput::Start)
            .await
            .unwrap();
        assert_eq!(catalog["connectionCount"], 1);
        assert_eq!(catalog["defaultTarget"]["modelId"], "fixture-model");
        assert_eq!(catalog["items"][0]["name"], "New connection");
        assert_eq!(catalog["items"][0]["enabledModelIdCount"], 1);
        assert_eq!(catalog["items"][0]["modelCount"], 2);
    });
    assert!(!String::from_utf8_lossy(&tui.output).contains("good-onboarding-secret"));
    tui.click_text("▤ Workspace");
    tui.wait_until(|s| s.lines().next().is_some_and(|l| l.contains("Workspace")));
    tui.send(b"\x0e");
    tui.wait_for("Message…");
    tui.wait_for("fixture-model");
    tui.send(b"hello after onboarding\x13");
    tui.wait_for("Onboarded model replied");
    runtime.block_on(provider).unwrap();
    tui.close_terminal();
    tui.finish();
    client.disconnect();
    host.retire_registered();
    assert!(host.wait_for_exit().success());
}

async fn serve(listener: tokio::net::TcpListener) {
    use serde_json::json;
    use tokio::io::AsyncWriteExt;
    for index in 0..3 {
        let (mut stream, request) = support::model_list_request(&listener).await;
        let (status, body) = if index == 0 {
            assert!(request.contains("Bearer wrong-onboarding-secret"));
            (
                "401 Unauthorized",
                json!({"error":"wrong-onboarding-secret"}),
            )
        } else {
            assert!(request.contains("Bearer good-onboarding-secret"));
            (
                "200 OK",
                json!({"data":[{"id":"fixture-model"},{"id":"unused-model"}]}),
            )
        };
        let body = body.to_string();
        stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    }
    let (mut stream, body) = model_request(&listener).await;
    assert!(body.to_string().contains("hello after onboarding"));
    let chunk = json!({"id":"onboarded","object":"chat.completion.chunk","model":"fixture-model",
        "choices":[{"index":0,"delta":{"content":"Onboarded model replied"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":123,"completion_tokens":4,"total_tokens":127}});
    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {chunk}\n\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
}
