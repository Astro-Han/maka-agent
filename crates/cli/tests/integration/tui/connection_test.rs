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

use super::enabled_models::catalog;
use super::*;
use maka_protocol::Operation;
use serde_json::json;

fn open(tui: &mut Pty) {
    tui.filter_command("Test connection");
    tui.click_text("Test connection");
    tui.wait_for("incur usage");
    tui.wait_for("Cancel");
}
#[test]
fn connection_test_confirms_network_records_failure_preserves_configuration_and_discards_stale_results()
 {
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
    let (client,listener,url,initial,default)=runtime.block_on(async {
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url=format!("http://{}/v1",listener.local_addr().unwrap());
        let client=support::model_client(&host.root,&url).await;
        client.create_session(maka_protocol::session::decode_session_create_input(&json!({"sessionId":"history","name":"History","workspace":{"kind":"host_path","path":directory.path()},"modelTarget":{"kind":"default"}})).unwrap()).await.unwrap();
        let (default,initial)=catalog(&client).await;
        (client,listener,url,initial,default)
    });
    let id = initial[0]["connectionId"].as_str().unwrap();
    let locator = json!({"scope":"connection","connectionId":id,"kind":"api_key"});
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.read_size = 128; // Never click a partially received command palette.
    tui.wait_for("Workspace");
    tui.click_text("⛭ Settings");
    tui.wait_for("Models"); // Connections live in the Models category.
    tui.click_text("Models");
    tui.wait_for("Model connections");
    tui.click_text("Model connections");
    // Settings also names the default connection; wait for the actual catalog row.
    tui.wait_for("› TUI fixture");
    tui.click_text("TUI fixture");
    open(&mut tui);
    tui.send(b"\r");
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("TUI fixture"));
    assert_eq!(runtime.block_on(catalog(&client)).1, initial);
    assert!(
        runtime.block_on(std::future::poll_fn(|cx| std::task::Poll::Ready(
            listener.poll_accept(cx).is_pending()
        )))
    );
    open(&mut tui);
    tui.click_last_text("Test connection");
    runtime.block_on(async {
        let (stream, body, headers) = model_http_request(&listener).await;
        assert_eq!(body["model"], "fixture-model");
        assert_eq!(body["max_tokens"], 16);
        assert!(body.get("stream").is_none());
        assert!(headers.contains("Bearer dummy-local-fixture"));
        support::json_response(
            stream,
            "401 Unauthorized",
            json!({"error":"do-not-display-provider-secret"}),
        )
        .await;
    });
    tui.wait_for("The service rejected the saved credentials.");
    tui.wait_for("HTTP 401");
    assert!(!String::from_utf8_lossy(&tui.output).contains("do-not-display-provider-secret"));
    runtime.block_on(async {
        let (current_default, items) = catalog(&client).await;
        assert_eq!(items[0]["revision"], 2);
        assert_eq!(items[0]["lastTest"]["status"], "needs_reauth");
        assert_eq!(current_default, default);
        assert_eq!(&items[1..], &initial[1..]);
    });
    tui.send(b"\t\r"); // The only terminal-result action is Close, never retry.
    tui.wait_until(|s| !s.contains("HTTP 401"));
    open(&mut tui);
    tui.click_last_text("Test connection");
    runtime.block_on(async {
        let (stream,_,_)=model_http_request(&listener).await;
        client.request(Operation::ConnectionCatalogUpdate,json!({"expected":{"connectionId":id,"revision":2},"changes":{"name":"Verified fixture","baseUrl":url,"enabled":true,"enabledModelIds":["fixture-model"]}})).await.unwrap();
        // The Host checks HTTP acceptance, not completion-body semantics.
        support::json_response(stream,"200 OK",json!({})).await;
    });
    tui.wait_for("Host check passed");
    tui.wait_for("fixture-model ·");
    runtime.block_on(async {
        let (current_default, items) = catalog(&client).await;
        assert_eq!(items[0]["revision"], 4);
        assert_eq!(items[0]["name"], "Verified fixture");
        assert_eq!(items[0]["lastTest"]["status"], "verified");
        assert_eq!(current_default, default);
        assert_eq!(&items[1..], &initial[1..]);
        assert_eq!(
            client.session("history").await.unwrap().unwrap().revision,
            1
        );
    });
    tui.send(b"\r");
    tui.wait_until(|s| !s.contains("Host check passed") && s.contains("Verified fixture"));
    open(&mut tui);
    tui.click_last_text("Test connection");
    let after_rotation=runtime.block_on(async {
        let (stream,_,_)=model_http_request(&listener).await;
        let key=client.request(Operation::CredentialVaultQuery,json!({"locator":locator})).await.unwrap();
        client.request(Operation::CredentialVaultSet,json!({"locator":locator,"expected":{"credentialId":key["status"]["credentialId"],"revision":key["status"]["revision"]},"secret":"rotated-probe-secret"})).await.unwrap();
        let state=catalog(&client).await;
        support::json_response(stream,"200 OK",json!({})).await;
        state
    });
    tui.wait_for("Connection, credentials or network settings changed.");
    assert_eq!(runtime.block_on(catalog(&client)), after_rotation);
    tui.send(b"\x1b");
    tui.wait_until(|s| !s.contains("Cancel"));
    open(&mut tui);
    tui.click_last_text("Test connection");
    let stream = runtime.block_on(async {
        let (stream, _, headers) = model_http_request(&listener).await;
        assert!(headers.contains("Bearer rotated-probe-secret"));
        stream
    });
    tui.click_text("⛭ Settings"); // Outside closes, but does not cancel or navigate through.
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("Verified fixture"));
    runtime.block_on(support::json_response(stream, "200 OK", json!({})));
    runtime.block_on(async {
        let until = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let (_, items) = catalog(&client).await;
            if items[0]["lastTest"]["status"] == "verified" {
                break;
            }
            assert!(tokio::time::Instant::now() < until);
            tokio::task::yield_now().await;
        }
    });
    // Establish a freshly loaded row, then change it with the menu held open.
    for name in ["Before menu open", "Updated while menu open"] {
        runtime.block_on(async {
            let (_, items) = catalog(&client).await;
            client.request(Operation::ConnectionCatalogUpdate,json!({
                "expected":{"connectionId":id,"revision":items[0]["revision"]},
                "changes":{"name":name,"baseUrl":url,"enabled":true,"enabledModelIds":["fixture-model"]}
            })).await.unwrap();
        });
        tui.wait_for(name);
        if name == "Before menu open" {
            tui.filter_command("Test connection");
        }
    }
    assert!(
        !tui.screen
            .snapshot()
            .unwrap()
            .screen
            .contains("Host check passed")
    );
    let updated = runtime.block_on(catalog(&client));
    tui.click_text("Test connection");
    tui.wait_for("incur usage");
    tui.send(b"\x1b");
    tui.wait_until(|s| !s.contains("incur usage"));
    assert_eq!(runtime.block_on(catalog(&client)), updated);
    tui.close_terminal();
    tui.finish();
    client.disconnect();
    host.retire_registered();
    assert!(host.wait_for_exit().success());
}
