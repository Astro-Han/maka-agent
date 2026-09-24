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

use super::support::json_response as reply;
use super::*;
use maka_protocol::{Operation, configuration::ConnectionCatalogQueryInput as Query};
use serde_json::{Value, json};

#[test]
fn model_fetch_confirms_network_writes_rechecks_effects_and_preserves_user_configuration() {
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
    let (client, listener, url, id, locator, initial) = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let client = support::model_client(&host.root, "http://127.0.0.1:9/v1").await;
        let created = client.request(Operation::ConnectionCatalogCreate, json!({
            "expectedCatalogRevision":2,
            "connection":{"slug":"discovery","name":"Discovery","providerType":"openai-compatible",
                "baseUrl":url,"enabled":true,"enabledModelIds":[],"requestBodyOverlay":{"temperature":0.3},
                "modelOverrides":{"first-model":{"contextWindow":64000}}}
        })).await.unwrap();
        let id = created["connection"]["connectionId"].as_str().unwrap().to_owned();
        let locator = json!({"scope":"connection","connectionId":id,"kind":"api_key"});
        client.request(Operation::CredentialVaultSet, json!({"locator":locator,"expected":null,
            "expectedConnection":{"connectionId":id,"revision":1,"slug":"discovery","providerType":"openai-compatible","effectiveBaseUrl":url},
            "secret":"discovery-secret"})).await.unwrap();
        let initial = client.connection_catalog(Query::Start).await.unwrap();
        (client, listener, url, id, locator, initial)
    });
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Workspace");
    tui.click_text("⛭  Settings");
    tui.wait_for("Models"); // Connections live in the Models category.
    tui.click_text("Models");
    tui.wait_for("Model connections");
    tui.click_text("Model connections");
    tui.wait_for("Discovery");
    tui.click_text("Discovery");
    open(&mut tui);
    tui.send(b"\r"); // Default Cancel must neither call the service nor write configuration.
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("Discovery"));
    assert_eq!(
        runtime
            .block_on(client.connection_catalog(Query::Start))
            .unwrap(),
        initial
    );
    assert!(
        runtime.block_on(std::future::poll_fn(|cx| std::task::Poll::Ready(
            listener.poll_accept(cx).is_pending()
        )))
    );

    open(&mut tui);
    tui.click_last_text("Fetch models");
    runtime.block_on(async {
        let (stream, request) = support::model_list_request(&listener).await;
        assert!(request.contains("Bearer discovery-secret"));
        reply(
            stream,
            "401 Unauthorized",
            json!({"error":"discovery-secret"}),
        )
        .await;
    });
    tui.wait_for("The service rejected the saved credentials.");
    assert!(!String::from_utf8_lossy(&tui.output).contains("discovery-secret"));
    assert_eq!(
        runtime
            .block_on(client.connection_catalog(Query::Start))
            .unwrap(),
        initial
    );

    tui.click_last_text("Fetch models"); // Explicit retry; rotate the credential while discovery is in flight.
    runtime.block_on(async {
        let (stream, _) = support::model_list_request(&listener).await;
        let status = client.request(Operation::CredentialVaultQuery, json!({"locator":locator})).await.unwrap();
        client.request(Operation::CredentialVaultSet, json!({"locator":locator,
            "expected":{"credentialId":status["status"]["credentialId"],"revision":status["status"]["revision"]},
            "secret":"rotated-discovery-secret"})).await.unwrap();
        reply(stream, "200 OK", json!({"data":[{"id":"stale-model"}]})).await;
    });
    tui.wait_for("Connection, credentials or network settings changed");
    assert_eq!(
        runtime
            .block_on(client.connection_catalog(Query::Start))
            .unwrap(),
        initial
    );
    tui.send(b"\x1b");
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("Discovery"));

    open(&mut tui);
    tui.click_last_text("Fetch models");
    runtime.block_on(async {
        let (stream, request) = support::model_list_request(&listener).await;
        assert!(request.contains("Bearer rotated-discovery-secret"));
        // Unrelated edits during the effect must survive its commit.
        client.request(Operation::ConnectionCatalogUpdate, json!({
            "expected":{"connectionId":id,"revision":1},
            "changes":{"name":"Discovery renamed","baseUrl":url,"enabled":true,"enabledModelIds":[]}
        })).await.unwrap();
        reply(
            stream,
            "200 OK",
            json!({"data":[{"id":"first-model"},{"id":"second-model"}]}),
        )
        .await;
    });
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("Discovery renamed"));
    runtime.block_on(async {
        let page = client.connection_catalog(Query::Start).await.unwrap();
        let row = row(&page, &id);
        assert_eq!(page["revision"], 5);
        assert_eq!(row["revision"], 3);
        assert_eq!(row["modelCount"], 2);
        assert_eq!(
            row["enabledModelIdCount"], 1,
            "empty connection enables only the first discovery"
        );
        assert_eq!(row["name"], "Discovery renamed");
        assert_eq!(row["requestBodyOverlay"], json!({"temperature":0.3}));
        assert_eq!(
            page["defaultTarget"], initial["defaultTarget"],
            "discovery does not select a Host default"
        );
        assert!(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["modelOverride"]["contextWindow"] == 64000)
        );
    });
    tui.click_text("Discovery renamed");
    open(&mut tui);
    tui.click_last_text("Fetch models");
    runtime.block_on(async {
        let (stream, _) = support::model_list_request(&listener).await;
        reply(stream, "200 OK", json!({"data":[{"id":"new-model"}]})).await;
    });
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("Discovery renamed"));
    runtime.block_on(async {
        let page = client.connection_catalog(Query::Start).await.unwrap();
        let current = row(&page, &id);
        assert_eq!(current["revision"], 4);
        assert_eq!(current["modelCount"], 1);
        let index = &current["connectionIndex"];
        let enabled: Vec<_> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["kind"] == "enabled_model_id" && item["connectionIndex"] == *index)
            .map(|item| item["modelId"].clone())
            .collect();
        assert_eq!(
            enabled,
            [json!("first-model")],
            "refresh must not replace enabled selections with the new inventory"
        );
        assert_eq!(page["defaultTarget"], initial["defaultTarget"]);
        let credential = client
            .request(Operation::CredentialVaultQuery, json!({"locator":locator}))
            .await
            .unwrap();
        assert_eq!(credential["status"]["revision"], 2);
    });
    tui.close_terminal();
    tui.finish();
    assert!(!String::from_utf8_lossy(&tui.output).contains("discovery-secret"));
    client.disconnect();
    host.retire_registered();
    assert!(host.wait_for_exit().success());
}

fn row<'a>(page: &'a Value, id: &str) -> &'a Value {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "connection" && item["connectionId"] == id)
        .unwrap()
}
fn open(tui: &mut Pty) {
    tui.filter_command("Fetch models");
    tui.click_text("Fetch models");
    tui.wait_for("Queries the currently configured service");
    tui.wait_for("Cancel");
}
