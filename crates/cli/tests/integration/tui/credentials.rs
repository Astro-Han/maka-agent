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
use maka_protocol::{configuration::*, session::*};
use serde_json::json;

#[test]
fn credential_management_rotates_with_cas_uses_the_new_key_and_clears_without_leaking() {
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
    let (client,locator,provider)=runtime.block_on(async {
        use tokio::io::AsyncWriteExt;
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url=format!("http://{}/v1",listener.local_addr().unwrap());
        let client=support::model_client(&host.root,&url).await;
        let catalog=client.connection_catalog(ConnectionCatalogQueryInput::Start).await.unwrap();
        let locator:CredentialLocator=serde_json::from_value(json!({"scope":"connection","connectionId":catalog["items"][0]["connectionId"],"kind":"api_key"})).unwrap();
        client.create_session(decode_session_create_input(&json!({
            "sessionId":"credential-chat","name":"Key verification","workspace":{"kind":"host_path","path":directory.path()},
            "modelTarget":{"kind":"default"}
        })).unwrap()).await.unwrap();
        let task=tokio::spawn(async move {
            let (mut stream,_,headers)=model_request_with_headers(&listener).await;
            assert!(headers.lines().any(|line|line.split_once(':').is_some_and(|(name,value)|
                name.eq_ignore_ascii_case("authorization")&&value.trim()=="Bearer new-terminal-key")));
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.unwrap();
            let chunk=json!({"id":"credential-test","object":"chat.completion.chunk","model":"fixture-model",
                "choices":[{"index":0,"delta":{"content":"New key accepted"},"finish_reason":"stop"}]});
            stream.write_all(format!("data: {chunk}\n\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
        });
        (client,locator,task)
    });
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Key verification");
    tui.click_text("⛭ Settings");
    tui.wait_for("Models"); // Connections live in the Models category.
    tui.click_text("Models");
    tui.wait_for("Model connections");
    tui.click_text("Model connections");
    tui.wait_for("TUI fixture");
    tui.click_text("TUI fixture");
    open(&mut tui, false);
    tui.wait_for("Key configured");
    tui.wait_for("New API key");
    tui.send(b"discarded-conflicting-key");
    runtime.block_on(async {
        let CredentialVaultQueryResult::Status { status } =
            client.credential_status(locator.clone()).await.unwrap()
        else {
            panic!()
        };
        let CredentialState::Configured {
            credential_id,
            revision,
            ..
        } = status.state
        else {
            panic!()
        };
        client
            .set_credential(SetCredentialInput {
                locator: locator.clone(),
                expected: Some(CredentialIdentityBasis {
                    credential_id,
                    revision,
                }),
                expected_connection: None,
                secret: "external-rotation-key".into(),
            })
            .await
            .unwrap();
    });
    tui.click_last_text("Save key");
    tui.wait_for("The connection or key changed elsewhere.");
    tui.send(b"\x1b");
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("TUI fixture"));
    open(&mut tui, false);
    tui.wait_for("Key configured");
    tui.wait_for("New API key");
    tui.send(b"new-terminal-key");
    tui.click_last_text("Save key");
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("TUI fixture"));
    runtime.block_on(async {
        let CredentialVaultQueryResult::Status { status } =
            client.credential_status(locator.clone()).await.unwrap()
        else {
            panic!()
        };
        assert!(matches!(
            status.state,
            CredentialState::Configured { revision: 3, .. }
        ));
        let catalog = client
            .connection_catalog(ConnectionCatalogQueryInput::Start)
            .await
            .unwrap();
        assert_eq!(
            catalog["revision"], 2,
            "key changes without prior test status do not rewrite the connection"
        );
        assert_eq!(
            client
                .session("credential-chat")
                .await
                .unwrap()
                .unwrap()
                .revision,
            1
        );
    });
    tui.click_text("Workspace");
    tui.wait_for("Key verification");
    tui.click_text("Key verification");
    tui.wait_for("Message…");
    tui.send(b"Verify saved key\x13");
    tui.wait_for("New key accepted");
    runtime.block_on(provider).unwrap();
    tui.click_text("⛭ Settings");
    tui.wait_for("Models"); // Connections live in the Models category.
    tui.click_text("Models");
    tui.wait_for("Model connections");
    tui.click_text("Model connections");
    tui.wait_for("TUI fixture");
    tui.click_text("TUI fixture");
    open(&mut tui, true);
    tui.wait_for("Key configured");
    tui.wait_for("Clear key");
    tui.send(b"\r");
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("TUI fixture"));
    runtime.block_on(async {
        assert!(matches!(
            client.credential_status(locator.clone()).await.unwrap(),
            CredentialVaultQueryResult::Status {
                status: CredentialStatus {
                    state: CredentialState::Configured { revision: 3, .. },
                    ..
                }
            }
        ));
    });
    open(&mut tui, true);
    tui.wait_for("Key configured");
    tui.wait_for("Clear key");
    tui.click_last_text("Clear key");
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("TUI fixture"));
    runtime.block_on(async {
        assert!(matches!(
            client.credential_status(locator.clone()).await.unwrap(),
            CredentialVaultQueryResult::Status {
                status: CredentialStatus {
                    state: CredentialState::Absent,
                    ..
                }
            }
        ));
    });
    open(&mut tui, true);
    tui.wait_for("No saved key");
    tui.click_last_text("Clear key");
    tui.send(b"\x1b");
    tui.wait_until(|s| !s.contains("Cancel"));
    open(&mut tui, false);
    tui.wait_for("No saved key");
    tui.send(b"never-persist-key-draft");
    tui.close_terminal();
    tui.finish();
    let checkpoint = directory
        .path()
        .join("tui-state")
        .join(&client.identity.root_id)
        .join("default/state.json");
    let saved = std::fs::read_to_string(checkpoint).unwrap();
    for secret in [
        "discarded-conflicting-key",
        "external-rotation-key",
        "new-terminal-key",
        "never-persist-key-draft",
    ] {
        assert!(!String::from_utf8_lossy(&tui.output).contains(secret));
        assert!(!saved.contains(secret));
    }
    client.disconnect();
    host.retire_registered();
    assert!(host.wait_for_exit().success());
}

fn open(tui: &mut Pty, clear: bool) {
    tui.filter_command("API key");
    if clear {
        tui.click_text("Clear API key");
    } else {
        // Match the standalone command rather than its substring in Clear API key.
        tui.click_text("API key");
    }
}
