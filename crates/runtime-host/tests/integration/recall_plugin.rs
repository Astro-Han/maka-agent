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

use super::{
    javascript_plugins::ready,
    support::{
        client_probe::ClientFixture,
        message_recovery::{ModelRequest, Provider, configure},
        peer::Peer,
    },
};
use maka_plugins::{
    composition::Scope,
    execution::{Progress, Submit},
    fiber::Fiber,
    kernel::Definition,
};
use maka_runtime::{configuration::policy::ChatDefaults, event::InvocationOutcome};
use maka_runtime_host::{
    plugins::Setup,
    server::{Host, HostOptions, local::LocalListener},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn renamed_recall_finds_archived_unicode_suffix_and_expands_without_echoing_current_turn() {
    tokio::time::timeout(Duration::from_secs(40), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-recall-");
    let (provider, mut requests) = Provider::controlled().await;
    let model = configure(&fixture, &provider.base_url).await;
    let config = maka_config::ConfigurationStore::for_root(Arc::new(fixture.owner()))
        .await
        .unwrap();
    config
        .set_chat_defaults(
            0,
            ChatDefaults {
                ..Default::default()
            },
        )
        .await
        .unwrap();
    config.close().await.unwrap();
    let source = format!(
        "{}CAFÉ late-needle\n{}",
        "Unrelated background 🦀.\n".repeat(1500),
        "Trailing details.\n".repeat(350)
    );
    let replies = tokio::spawn(async move {
        finish(requests.recv().await.unwrap(), &source);
        reply(
            requests.recv().await.unwrap(),
            "tool_search",
            json!({"query":"Recall RecallMore"}),
        );
        reply(
            requests.recv().await.unwrap(),
            "Recall",
            json!({"terms":["cafe\u{301}","LATE-NEEDLE"]}),
        );
        let request = requests.recv().await.unwrap();
        let text = latest_tool(&request.body);
        assert!(text.contains("CAFÉ late-needle"), "{text}");
        assert!(text.contains("(source)"), "{text}");
        assert!(
            !text.contains("(search)"),
            "current Turn echoed as corroboration: {text}"
        );
        let anchor = text
            .lines()
            .find_map(|line| line.strip_prefix("Anchor: "))
            .unwrap()
            .to_owned();
        let offset = text
            .lines()
            .find_map(|line| {
                let (_, rest) = line.split_once("next Some(")?;
                rest.split_once(')')?.0.parse::<usize>().ok()
            })
            .unwrap();
        reply(
            request,
            "RecallMore",
            json!({"session_id":"source","anchor_message_id":anchor,"before":0,"after":0,"offset":offset}),
        );
        let request = requests.recv().await.unwrap();
        let text = latest_tool(&request.body);
        assert!(text.contains("Trailing details."), "{text}");
        assert!(!text.contains("Unrelated background"), "{text}");
        finish(request, "Recovered the archived exchange");
    });
    let host = Host::open_with_options(
        fixture.owner(),
        None,
        HostOptions {
            plugins: setup(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("recall.sock");
    #[cfg(windows)]
    let endpoint =
        std::path::PathBuf::from(format!(r"\\.\pipe\maka-recall-{}", uuid::Uuid::new_v4()));
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop),
    );
    let mut peer = Peer::new(host.clone(), "recall").await;
    ready(&mut peer).await;
    for session in ["source", "search"] {
        success(peer.rpc("session.create", json!({
            "sessionId":session,"workspace":{"kind":"host_path","path":fixture.workspace},
            "sandboxMode":"danger-full-access","modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model}
        })).await);
    }
    run(&host, "source", "Remember the exchange").await;
    success(
        peer.rpc(
            "session.lifecycle.set",
            json!({"sessionId":"source","state":"archived"}),
        )
        .await,
    );
    run(
        &host,
        "search",
        "Find CAFÉ late-needle from earlier history",
    )
    .await;
    replies.await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 5);
    peer.close().await;
    drop(cleanup);
    server.await.unwrap().unwrap();
}
async fn run(host: &Arc<Host>, session: &str, content: &str) {
    let driver = Fiber::new("example.driver", session, Scope::Profile).unwrap();
    driver.begin_loading().unwrap();
    let commands = host
        .authorize_plugin_execution(driver.context(), &[session.into()])
        .await
        .unwrap();
    driver.ready().unwrap();
    driver.publish().unwrap();
    commands
        .submit(Submit {
            orchestration_mode: None,
            operation_id: session.into(),
            session_id: session.into(),
            content: content.into(),
        })
        .await
        .unwrap();
    loop {
        if let Progress::Ended { outcome } = commands.query(session.into()).await.unwrap().progress
        {
            assert_eq!(outcome, InvocationOutcome::Completed);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    driver
        .shutdown(tokio::time::Instant::now() + Duration::from_secs(2))
        .await
        .unwrap();
}
fn latest_tool(body: &Value) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .into()
}
fn reply(request: ModelRequest, name: &str, input: Value) {
    request.reply.send(json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("call-{name}"),"type":"function","function":{"name":name,"arguments":input.to_string()}}]},"finish_reason":"tool_calls"})).unwrap();
}
fn finish(request: ModelRequest, text: &str) {
    request
        .reply
        .send(json!({"index":0,"delta":{"content":text},"finish_reason":"stop"}))
        .unwrap();
}
fn success(value: Value) -> Value {
    assert_eq!(value["ok"], true, "{value}");
    value["result"].clone()
}
fn setup() -> Setup {
    let id = "z.history";
    Setup {
        builtins: [(id.into(), Arc::new(Definition { id:id.into(), revision:"binary".into(), dependencies:vec![], inject:vec![], plugin:Arc::new(maka_assistant::recall::Builtin) }))].into(),
        layers: [(id.into(), vec![
            serde_json::from_value(json!({"type":"remove","entryId":"maka.recall"})).unwrap(),
            serde_json::from_value(json!({"type":"insert","rootId":"profile","entry":{"id":"recall","packageId":id}})).unwrap(),
        ])].into(), ..Default::default()
    }
}
