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

#[path = "oauth/support.rs"]
mod support;
mod verification;
use maka_config::{
    ConfigurationStore,
    oauth::enrollment::{LoginCompletion, LoginPreparation},
};
use maka_runtime::oauth::{LoginStart, Provider, Target};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use support::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initiating_client_admission_replay_disconnect_cancel_and_drain_own_authorization() {
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Fixture::new(Some(proxy.local_addr().unwrap().port())).await;
    let (host, _drain, server) = fixture.serve().await;
    let mut first = Peer::new(host.clone(), "oauth-first").await;
    let mut second = Peer::new(host.clone(), "oauth-second").await;
    first.publish("first-publication").await;
    let refused = second
        .rpc("oauth.login.start", start("missing-service"))
        .await;
    assert_eq!(
        refused["error"]["code"], "capability_unavailable",
        "{refused}"
    );
    let enabled = second
        .rpc("oauth.enrollment.query", json!({"provider":"xai-oauth"}))
        .await;
    assert_eq!(
        enabled["result"],
        json!({"provider":"xai-oauth","enabled":true})
    );
    let started = first.rpc("oauth.login.start", start("active")).await;
    assert_eq!(
        started["result"]["phase"], "awaiting_authorization",
        "{started}"
    );
    let mut socket = pending_connect(&proxy).await;
    let repeated = second.rpc("oauth.login.start", start("active")).await;
    assert_eq!(repeated, started);
    let other = second.rpc("oauth.login.start", start("other")).await;
    assert_eq!(other["error"]["code"], "operation_conflict", "{other}");
    let mismatch = second
        .rpc(
            "oauth.login.start",
            json!({"attemptId":"active",
        "target":{"kind":"create","providerType":"openai-codex"}}),
        )
        .await;
    assert_eq!(mismatch["error"]["code"], "invalid_request", "{mismatch}");
    first.close().await;
    let queried = second
        .rpc("oauth.login.query", json!({"attemptId":"active"}))
        .await;
    assert_eq!(
        queried["result"], started["result"],
        "disconnect must not abandon authorization"
    );
    let diagnostics = second.rpc("host.diagnostics.query", json!({})).await;
    assert_eq!(diagnostics["result"]["connections"], 1);
    assert_eq!(diagnostics["result"]["upgradeBlockingActivity"], true);
    assert_eq!(
        diagnostics["result"]["residencies"],
        json!([{"label":"oauth", "count":1}])
    );
    let epoch = diagnostics["result"]["hostEpoch"].clone();
    for cooperate in [false, true] {
        let refused = second
            .rpc(
                "host.upgrade.prepare",
                json!({
                    "expectedHostEpoch":epoch, "allowInterruptActiveTasks":false,
                    "allowCooperativeHandoff":cooperate,
                }),
            )
            .await;
        assert_eq!(refused["result"], json!({"kind":"active_tasks"}));
    }
    let cancelled = second
        .rpc("oauth.login.cancel", json!({"attemptId":"active"}))
        .await;
    assert!(
        matches!(
            cancelled["result"]["phase"].as_str(),
            Some("awaiting_authorization" | "cancelled")
        ),
        "{cancelled}"
    );
    assert_eq!(
        cancelled["result"]["connection"],
        started["result"]["connection"]
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), socket.read_u8())
            .await
            .unwrap()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    second.publish("second-publication").await;
    // A cancelled projection can precede actual transport cleanup. Admission
    // remains closed until that owner has completely settled.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let next = second.rpc("oauth.login.start", start("draining")).await;
        if next["ok"] == true {
            break;
        }
        assert_eq!(next["error"]["code"], "operation_conflict", "{next}");
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    let settled = second
        .rpc("oauth.login.query", json!({"attemptId":"active"}))
        .await;
    assert_eq!(settled["result"]["phase"], "cancelled");
    let mut socket = pending_connect(&proxy).await;
    let retired = second
        .rpc(
            "host.upgrade.prepare",
            json!({
                "expectedHostEpoch":epoch, "allowInterruptActiveTasks":true,
            }),
        )
        .await;
    assert_eq!(
        retired["result"],
        json!({"kind":"prepared", "pid":std::process::id()})
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), socket.read_u8())
            .await
            .unwrap()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    second.close().await;
    finish(server).await;
    drop(host);
    let store = ConfigurationStore::for_root(Arc::new(fixture.owner()))
        .await
        .unwrap();
    assert!(
        store.catalog().await.unwrap().connections.is_empty(),
        "cancelled logins publish no drafts"
    );
    assert!(
        store
            .oauth_login_receipt("active".into())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .oauth_login_receipt("draining".into())
            .await
            .unwrap()
            .is_none()
    );
    store.close().await.unwrap();
}
async fn pending_connect(proxy: &tokio::net::TcpListener) -> BufReader<tokio::net::TcpStream> {
    let (socket, _) = tokio::time::timeout(Duration::from_secs(5), proxy.accept())
        .await
        .unwrap()
        .unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(line, "CONNECT auth.x.ai:443 HTTP/1.1\r\n");
    loop {
        line.clear();
        socket.read_line(&mut line).await.unwrap();
        if line == "\r\n" {
            break;
        }
        assert!(line.len() < 8192 && !line.is_empty());
    }
    socket
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_provider_durable_login_receipts_replay_without_presentation_after_reopen() {
    let fixture = Fixture::new(None).await;
    let store = Arc::new(
        ConfigurationStore::for_root(Arc::new(fixture.owner()))
            .await
            .unwrap(),
    );
    let mut saved = Vec::new();
    for provider in [
        Provider::OpenaiCodex,
        Provider::GithubCopilot,
        Provider::XaiOauth,
    ] {
        let input = LoginStart {
            attempt_id: provider.as_str().into(),
            target: Target::Create {
                provider_type: provider,
                slug: None,
                name: None,
            },
        };
        let LoginPreparation::Ready(ticket) =
            store.prepare_oauth_login(input.clone()).await.unwrap()
        else {
            panic!("expected unpublished ticket");
        };
        let identity = ticket.identity().clone();
        assert!(matches!(
            ticket
                .complete("synthetic-receipt-fixture".into(), 1)
                .await
                .unwrap(),
            LoginCompletion::Committed(_)
        ));
        saved.push((input, identity));
    }
    store.shutdown().await.unwrap();
    drop(store);
    for _ in 0..2 {
        let (host, drain, server) = fixture.serve().await;
        let mut command = tokio::process::Command::new("node");
        command
            .kill_on_drop(true)
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../tests/fixtures/client.mjs"),
            )
            .arg("--socket")
            .arg(&fixture.endpoint)
            .args(["--root-id", host.root_id(), "--oauth-receipts"])
            .arg(serde_json::to_string(&saved).unwrap());
        let output = tokio::time::timeout(Duration::from_secs(30), command.output()).await;
        drain.cancel();
        finish(server).await;
        drop(host);
        let output = output.unwrap().unwrap();
        assert!(
            output.status.success(),
            "original client failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("\"check\":\"oauth-receipts\""));
    }
}
