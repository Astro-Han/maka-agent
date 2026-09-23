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
use maka_protocol::{
    Operation,
    plugin::{RemoteBinding, RemoteRequest, RemoteResult},
};
use serde_json::{Value, json};

#[test]
fn scheduler_form_edits_multiline_and_fences_stale_writes_without_running_a_model() {
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
    let (client, listener, id) = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = support::model_client(
            &host.root,
            &format!("http://{}/v1", listener.local_addr().unwrap()),
        )
        .await;
        // Setup uses the existing consent authority, not a second task store.
        let binding = RemoteBinding::Package { package_id: "maka.scheduler".into(), method: "terminal".into(), session_id: None };
        let RemoteResult::Bound { target, .. } = client.plugin_remote(RemoteRequest::Bind { binding: binding.clone() }).await.unwrap() else { panic!("terminal") };
        let grant = client
            .request(
                Operation::PluginAuthorization,
                json!({
                    "binding":binding, "target":target, "command":{"kind":"approve", "request":{
                        "operationId":uuid::Uuid::new_v4(), "title":"Fixture reminder",
                        "target":{"kind":"profile"}, "capabilities":["notifications"]
                    }}
                }),
            )
            .await
            .unwrap();
        let run_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 3_600_000;
        let task = remote(
            &client,
            "request",
            json!({"kind":"mutate","grant":grant["grant"]["id"],
                "mutation":{"kind":"create","input":{"title":"Scheduled fixture",
                "intentBody":"Original note","schedule":{"kind":"interval","everySeconds":600,"startAt":run_at},
                    "effect":{"kind":"notify","channel":"local"}}}
            }),
        )
        .await;
        let id = task["task"]["id"].as_str().unwrap().to_owned();
        (client, listener, id)
    });
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Workspace");
    tui.filter_command("Plugin pages");
    tui.click_text("Plugin pages");
    tui.wait_for("Scheduled tasks");
    tui.click_text("Scheduled tasks");
    tui.wait_for("Scheduled fixture");
    tui.click_text("Scheduled fixture");
    tui.wait_for("Original note");
    tui.click_text("Original note");
    tui.send(b"\x01");
    tui.send(b"\x1b[200~First line\x1b[201~");
    tui.send(b"\rSecond line");
    tui.wait_for("Second line");
    tui.send(b"\x1b[A\x1b[B"); // Vertical movement belongs to the text editor.
    tui.click_text("Save");
    tui.wait_for("✓ Save");
    let query = || json!({"kind":"query","query":{"kind":"get","taskId":id}});
    let first = runtime.block_on(remote(&client, "request", query()));
    assert_eq!(first["task"]["intent"]["body"], "First line\nSecond line");
    assert_eq!(first["task"]["status"], "active");
    tui.click_text("Pause");
    tui.wait_for("Resume");
    let paused = runtime.block_on(remote(&client, "request", query()));
    assert_eq!(paused["task"]["status"], "paused");
    tui.click_text("Resume");
    tui.wait_for("Pause");
    tui.click_text("Interval");
    tui.wait_for("Every (seconds)");
    tui.click_text("Every (seconds)");
    tui.send(b"\x01\x1b[200~9\x1b[201~");
    tui.click_text("Save");
    tui.wait_for("Check the date, UTC offset and recurrence fields");
    tui.click_text("Every (seconds)");
    tui.send(b"\x01\x1b[200~900\x1b[201~");
    tui.click_text("Save");
    tui.wait_for("✓ Save");
    let rescheduled = runtime.block_on(remote(&client, "request", query()));
    assert_eq!(rescheduled["task"]["schedule"]["everySeconds"], 900);
    assert_eq!(
        rescheduled["task"]["schedule"]["startAt"],
        first["task"]["schedule"]["startAt"]
    );
    tui.send(b"\x1b");
    tui.wait_for("First line");
    runtime.block_on(remote(
        &client,
        "request",
        json!({"kind":"mutate","mutation":{
            "kind":"update","taskId":id,"patch":{"title":"Changed elsewhere"}
        }}),
    ));
    tui.click_text("First line");
    tui.send(b"\x01\x1b[200~Keep this draft\x1b[201~");
    tui.click_text("Save");
    tui.wait_for("Your draft is preserved");
    tui.wait_for("Keep this draft");
    tui.send(b"\r");
    let actual = runtime.block_on(remote(&client, "request", query()));
    assert_eq!(actual["task"]["title"], "Changed elsewhere");
    assert_eq!(actual["task"]["intent"]["body"], "First line\nSecond line");
    assert_eq!(actual["task"]["fireCount"], 0);
    tui.send(b"\x11");
    tui.finish();
    runtime.block_on(async {
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        // Remove the pending fixture through its original domain operation.
        remote(
            &client,
            "request",
            json!({"kind":"mutate","mutation":{"kind":"delete","taskId":id}}),
        )
        .await;
    });
    client.disconnect();
}

#[test]
fn scheduler_creation_requires_consent_then_reuses_only_a_live_grant() {
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
    let (client, listener) = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = support::model_client(
            &host.root,
            &format!("http://{}/v1", listener.local_addr().unwrap()),
        )
        .await;
        client
            .create_session(
                maka_protocol::session::decode_session_create_input(&json!({
                    "sessionId":"reminder-source", "name":"Reminder source",
                    "workspace":{"kind":"host_path","path":directory.path()},
                    "sandboxMode":"read-only", "modelTarget":{"kind":"default"}
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        (client, listener)
    });
    let list = || {
        runtime.block_on(remote(
            &client,
            "request",
            json!({"kind":"query","query":{"kind":"list"}}),
        ))
    };
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Reminder source");
    tui.click_text("Reminder source");
    tui.wait_for("fixture-model");
    tui.filter_command("Plugin pages");
    tui.click_text("Plugin pages");
    tui.wait_for("Scheduled tasks");
    tui.click_text("Scheduled tasks");
    tui.wait_for("New reminder");
    tui.click_text("New reminder");
    tui.wait_for("Interval");
    tui.click_text("Interval");
    tui.wait_for("Every (seconds)");
    tui.click_text("Title");
    tui.send(b"\x1b[200~Consent fixture\x1b[201~");
    tui.click_text("Content");
    tui.send(b"\x1b[200~First reminder\x1b[201~");
    tui.click_text("Create reminder");
    tui.wait_for("Allow plugin access?");
    tui.send(b"\r"); // Default focus cancels; no authorization or task mutation.
    tui.wait_until(|screen| {
        !screen.contains("Allow plugin access?") && screen.contains("First reminder")
    });
    assert!(list()["tasks"].as_array().unwrap().is_empty());
    assert!(
        runtime
            .block_on(remote(&client, "request", json!({"kind":"grants"})))
            .as_array()
            .unwrap()
            .is_empty()
    );
    tui.click_text("Create reminder");
    tui.wait_for("Allow plugin access?");
    tui.click_text("Allow and continue");
    tui.wait_for("Pause");
    let tasks = list();
    let task = &tasks["tasks"][0];
    assert_eq!(tasks["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(task["title"], "Consent fixture");
    assert_eq!(task["intent"]["body"], "First reminder");
    assert_eq!(task["schedule"]["everySeconds"], 3600);
    assert_eq!(task["effect"], json!({"kind":"notify","channel":"local"}));
    let grants = runtime.block_on(remote(&client, "request", json!({"kind":"grants"})));
    assert_eq!(grants.as_array().unwrap().len(), 1);

    // Fresh reads keep creation identity separate while reusing actual authority.
    tui.send(b"\x1b");
    tui.wait_for("maka.scheduler");
    tui.click_text("Scheduled tasks");
    tui.wait_for("New reminder");
    tui.click_text("New reminder");
    tui.wait_for("Once");
    tui.click_text("Once");
    tui.wait_for("Content");
    tui.click_text("Title");
    tui.send(b"\x1b[200~Second reminder\x1b[201~");
    tui.click_text("Content");
    tui.send(b"\x1b[200~No second consent\x1b[201~");
    tui.click_text("Create reminder");
    tui.wait_for("Pause");
    assert_eq!(list()["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(
        runtime.block_on(remote(&client, "request", json!({"kind":"grants"}))),
        grants
    );
    tui.send(b"\x11");
    tui.finish();
    runtime.block_on(async {
        let binding = RemoteBinding::Package { package_id: "maka.scheduler".into(), method: "terminal".into(), session_id: None };
        let RemoteResult::Bound { target, .. } = client.plugin_remote(RemoteRequest::Bind { binding: binding.clone() }).await.unwrap() else { panic!("terminal") };
        client.request(Operation::PluginAuthorization, json!({"binding":binding,"target":target,
            "command":{"kind":"revoke","id":grants[0]["id"]}})).await.unwrap();
        let page = remote(&client, "terminal", json!({"kind":"read","route":{"creation":{"kind":"form","schedule":"daily"}}})).await;
        let mut fields = page["page"]["fields"].as_array().unwrap().iter().map(|field| (field["id"].as_str().unwrap().to_owned(), field["control"]["value"].clone())).collect::<serde_json::Map<_, _>>();
        fields.insert("title".into(), json!("Needs fresh consent"));
        fields.insert("intent".into(), json!("Revoked authority cannot be reused"));
        let result = remote(&client, "terminal", json!({"kind":"submit", "route":{"creation":{"kind":"form","schedule":"daily"}}, "revision":page["page"]["revision"], "action":"create", "fields":fields})).await;
        assert_eq!(result["kind"], "consent");
        let tasks = remote(&client, "request", json!({"kind":"query","query":{"kind":"list"}})).await;
        assert_eq!(tasks["tasks"].as_array().unwrap().len(), 2);
        for task in tasks["tasks"].as_array().unwrap() {
            remote(&client, "request", json!({"kind":"mutate","mutation":{"kind":"delete","taskId":task["id"]}})).await;
        }
        assert!(tokio::time::timeout(Duration::from_millis(50), listener.accept()).await.is_err());
    });
    client.disconnect();
}

async fn remote(client: &maka_client::Client, method: &str, input: Value) -> Value {
    let binding = RemoteBinding::Package {
        package_id: "maka.scheduler".into(),
        method: method.into(),
        session_id: None,
    };
    let RemoteResult::Bound { target, .. } = client
        .plugin_remote(RemoteRequest::Bind {
            binding: binding.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("bound")
    };
    let RemoteResult::Document { document } = client
        .plugin_remote(RemoteRequest::OpenDocument)
        .await
        .unwrap()
    else {
        panic!("document")
    };
    let result = client
        .plugin_remote(RemoteRequest::Call {
            binding,
            target,
            document,
            input,
        })
        .await
        .unwrap();
    client
        .plugin_remote(RemoteRequest::CloseDocument { document })
        .await
        .unwrap();
    let RemoteResult::Value { value } = result else {
        panic!("value")
    };
    value
}
