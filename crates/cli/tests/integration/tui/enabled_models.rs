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
use maka_protocol::{Operation, configuration::ConnectionCatalogQueryInput as Query};
use serde_json::{Value, json};

pub(super) async fn catalog(client: &maka_client::Client) -> (Value, Vec<Value>) {
    let mut query = Query::Start;
    let mut items = vec![];
    loop {
        let page = client.connection_catalog(query).await.unwrap();
        items.extend(page["items"].as_array().unwrap().iter().cloned());
        if page["nextCursor"].is_null() {
            return (page["defaultTarget"].clone(), items);
        }
        query = Query::Continue {
            revision: page["revision"].as_u64().unwrap(),
            cursor: serde_json::from_value(page["nextCursor"].clone()).unwrap(),
        };
    }
}
fn ids(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter(|item| item["kind"] == "enabled_model_id")
        .map(|item| item["modelId"].as_str().unwrap().to_owned())
        .collect()
}
fn open(tui: &mut Pty) {
    tui.filter_command("Enabled models");
    tui.click_text("Enabled models");
    tui.wait_for("Search models");
    tui.wait_for("130 enabled.");
}

#[test]
fn enabled_models_searches_full_catalog_keeps_manual_ids_and_applies_only_confirmed_sets() {
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
    let (client, original, credential) = runtime.block_on(async {
        let client = support::model_client(&host.root,"http://127.0.0.1:9/v1").await;
        let (_,items) = catalog(&client).await;
        let row = &items[0];
        let enabled:Vec<_> = std::iter::once("fixture-model".to_owned()).chain((0..129).map(|i| format!("manual-{i:03}"))).collect();
        let result = client.request(Operation::ConnectionCatalogUpdate,json!({
            "expected":{"connectionId":row["connectionId"],"revision":row["revision"]},
            "changes":{"name":"Enabled fixture","baseUrl":"http://127.0.0.1:9/v1","enabled":true,"enabledModelIds":enabled,
                "modelOverrides":{"fixture-model":{"contextWindow":128000},"new-model":{"contextWindow":64000,"displayName":"New choice"}},"requestBodyOverlay":{"temperature":0.2}}
        })).await.unwrap();
        client.create_session(maka_protocol::session::decode_session_create_input(&json!({"sessionId":"history","name":"Existing history",
            "workspace":{"kind":"host_path","path":directory.path()},"modelTarget":{"kind":"default"}})).unwrap()).await.unwrap();
        let credential = client.request(Operation::CredentialVaultQuery,json!({"locator":{"scope":"connection","connectionId":row["connectionId"],"kind":"api_key"}})).await.unwrap();
        (client,result["connection"].clone(),credential)
    });
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Workspace");
    tui.click_text("⛭ Settings");
    tui.wait_for("Model connections");
    tui.click_text("Model connections");
    tui.wait_for("Enabled fixture");
    tui.click_text("Enabled fixture");
    open(&mut tui);
    tui.send(b"manual-128");
    tui.wait_until(|s| {
        s.contains("⌕ manual-128") && s.contains("[✓] manual-128") && !s.contains("[✓] manual-127")
    });
    tui.click_last_text("manual-128");
    tui.wait_for("129 enabled.");
    tui.send(b"\x1b[Z\x01new-model");
    tui.wait_for("[ ] new-model");
    tui.click_last_text("new-model");
    tui.wait_for("130 enabled.");
    tui.click_last_text("Save");
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("Enabled fixture"));
    let saved = runtime.block_on(async {
        let (default,items) = catalog(&client).await;
        let enabled = ids(&items);
        assert_eq!(enabled.len(),130); assert!(enabled.contains(&"new-model".into())); assert!(!enabled.contains(&"manual-128".into()));
        assert!(enabled.contains(&"manual-127".into()),"undiscovered manual models survive full pagination");
        assert_eq!(default["modelId"],"fixture-model");
        assert_eq!(items[0]["revision"],original["revision"].as_u64().unwrap()+1);
        assert_eq!(items[0]["requestBodyOverlay"]["temperature"],0.2);
        assert!(items.iter().any(|item| item["entry"]["id"]=="new-model" && item["entry"]["contextWindow"]==64000));
        assert_eq!(client.request(Operation::CredentialVaultQuery,json!({"locator":{"scope":"connection","connectionId":original["connectionId"],"kind":"api_key"}})).await.unwrap(),credential);
        assert_eq!(client.session("history").await.unwrap().unwrap().revision,1);
        items[0].clone()
    });
    open(&mut tui);
    tui.send(b"new-model");
    tui.wait_for("[✓] new-model");
    tui.click_last_text("new-model");
    tui.wait_for("129 enabled.");
    runtime.block_on(async {
        let (_,items)=catalog(&client).await;
        client.request(Operation::ConnectionCatalogUpdate,json!({"expected":{"connectionId":saved["connectionId"],"revision":saved["revision"]},
            "changes":{"name":"Renamed elsewhere","baseUrl":"http://127.0.0.1:9/v1","enabled":true,"enabledModelIds":ids(&items)}})).await.unwrap();
    });
    tui.click_last_text("Save");
    tui.wait_for("This connection changed");
    tui.send(b"\x1b");
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("Renamed elsewhere"));
    tui.click_text("Renamed elsewhere");
    open(&mut tui);
    tui.send(b"fixture-model");
    tui.wait_for("⌕ fixture-model");
    tui.wait_for("[✓] fixture-model");
    tui.click_last_text("fixture-model");
    tui.wait_for("clears the Host default");
    tui.click_text("⛭ Settings"); // Outside closes only the modal, no write or navigation.
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("Renamed elsewhere"));
    runtime.block_on(async {
        assert_eq!(catalog(&client).await.0["modelId"], "fixture-model");
    });
    open(&mut tui);
    tui.send(b"fixture-model");
    tui.wait_for("⌕ fixture-model");
    tui.wait_for("[✓] fixture-model");
    tui.click_last_text("fixture-model");
    tui.wait_for("clears the Host default");
    tui.click_last_text("Save");
    tui.wait_until(|s| !s.contains("Cancel") && s.contains("129 enabled models"));
    runtime.block_on(async {
        let (default, items) = catalog(&client).await;
        assert!(default.is_null());
        assert!(!ids(&items).contains(&"fixture-model".into()));
        assert!(ids(&items).contains(&"new-model".into()));
        assert_eq!(
            client.session("history").await.unwrap().unwrap().model,
            "fixture-model"
        );
        assert_eq!(
            client.session("history").await.unwrap().unwrap().revision,
            1
        );
        assert_eq!(items[0]["requestBodyOverlay"]["temperature"], 0.2);
    });
    tui.close_terminal();
    tui.finish();
    client.disconnect();
    host.retire_registered();
    assert!(host.wait_for_exit().success());
}
