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
        message_recovery::{Provider, configure},
        peer::Peer,
    },
};
use maka_plugins::{
    client::Bundle,
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
async fn renamed_todo_keeps_session_documents_paged_live_and_durable_across_retirement() {
    tokio::time::timeout(Duration::from_secs(40), scenario())
        .await
        .unwrap();
}

async fn scenario() {
    let fixture = ClientFixture::new("maka-todo-");
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
    let items: Vec<Value> = (0..200)
        .map(|index| {
            json!({
                "content": format!("{index:03} {}", "🦀".repeat(196)),
                "status":if index % 2 == 0 { "completed" } else { "pending" },
            })
        })
        .collect();
    let expected = items.clone();
    let replies = tokio::spawn(async move {
        let first = requests.recv().await.unwrap();
        reply(first, "tool_search", json!({"query":"todo checklist"}));
        let second = requests.recv().await.unwrap();
        assert!(
            second.body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == "todo_write"),
            "{}",
            second.body
        );
        reply(second, "todo_write", json!({"todos":items}));
        let third = requests.recv().await.unwrap();
        reply(third, "todo_read", json!({}));
        let fourth = requests.recv().await.unwrap();
        fourth
            .reply
            .send(json!({"index":0,"delta":{"content":"Checklist ready"},"finish_reason":"stop"}))
            .unwrap();
    });
    for reopened in [false, true] {
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
        let endpoint = fixture.workspace.parent().unwrap().join("todo.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-todo-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop),
        );
        let mut peer = Peer::new(host.clone(), "todo").await;
        ready(&mut peer).await;
        if !reopened {
            for session in ["checklist", "other"] {
                success(peer.rpc("session.create", json!({
                    "sessionId":session, "workspace":{"kind":"host_path","path":fixture.workspace},
                    "sandboxMode":"danger-full-access", "modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model}
                })).await);
            }
        }
        let (document, stream) = watch(&mut peer, "checklist").await;
        if !reopened {
            assert_eq!(
                snapshot(&mut peer, &document, &stream).await,
                Vec::<Value>::new()
            );
            let driver = Fiber::new("example.driver", "driver", Scope::Profile).unwrap();
            driver.begin_loading().unwrap();
            let commands = host
                .authorize_plugin_execution(driver.context(), &["checklist".into()])
                .await
                .unwrap();
            driver.ready().unwrap();
            driver.publish().unwrap();
            commands
                .submit(Submit {
                    orchestration_mode: None,
                    operation_id: "checklist".into(),
                    session_id: "checklist".into(),
                    content: "Make a checklist".into(),
                })
                .await
                .unwrap();
            loop {
                if let Progress::Ended { outcome } =
                    commands.query("checklist".into()).await.unwrap().progress
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
        assert_eq!(snapshot(&mut peer, &document, &stream).await, expected);
        let (other_document, other_stream) = watch(&mut peer, "other").await;
        assert!(
            snapshot(&mut peer, &other_document, &other_stream)
                .await
                .is_empty()
        );
        success(
            peer.rpc(
                "plugin.remote",
                json!({"kind":"close_document","document":other_document}),
            )
            .await,
        );
        if !reopened {
            success(peer.rpc("plugin.composition.apply", json!({"operations":[{"type":"update","entryId":"checklist-backend","patch":{"disabled":true}}]})).await);
            // The dependent UI withdraws with the backend. Old streams never retarget.
            loop {
                let clients = success(
                    peer.rpc("plugin.client.query", json!({"kind":"snapshot"}))
                        .await,
                );
                if clients["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|entry| entry["extensionId"] != "z.checklist")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(
                peer.rpc(
                    "plugin.remote",
                    json!({"kind":"next","document":document,"stream":stream})
                )
                .await["ok"],
                false
            );
            success(peer.rpc("plugin.composition.apply", json!({"operations":[{"type":"update","entryId":"checklist-backend","patch":{"disabled":false}}]})).await);
            ready(&mut peer).await;
            let (fresh_document, fresh_stream) = watch(&mut peer, "checklist").await;
            assert_eq!(
                snapshot(&mut peer, &fresh_document, &fresh_stream).await,
                expected
            );
            success(
                peer.rpc(
                    "plugin.remote",
                    json!({"kind":"close_document","document":fresh_document}),
                )
                .await,
            );
        }
        success(
            peer.rpc(
                "plugin.remote",
                json!({"kind":"close_document","document":document}),
            )
            .await,
        );
        peer.close().await;
        drop(cleanup);
        server.await.unwrap().unwrap();
    }
    replies.await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 4);
}

fn reply(request: super::support::message_recovery::ModelRequest, name: &str, input: Value) {
    request.reply.send(json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("call-{name}"),"type":"function","function":{"name":name,"arguments":input.to_string()}}]},"finish_reason":"tool_calls"})).unwrap();
}
fn setup() -> Setup {
    let id = "z.checklist";
    Setup {
        builtins: [(id.into(), Arc::new(Definition {
            id:id.into(), revision:"binary".into(), dependencies:vec![], inject:vec![],
            plugin:Arc::new(maka_assistant::todo::Builtin { client:Some(Bundle::builtin(id, "binary", "export default function() {}").unwrap()) }),
        }))].into(),
        layers: [(id.into(), vec![
            serde_json::from_value(json!({"type":"remove","entryId":"maka.todo.ui"})).unwrap(),
            serde_json::from_value(json!({"type":"remove","entryId":"maka.todo"})).unwrap(),
            serde_json::from_value(json!({"type":"insert","rootId":"profile","entry":{"id":"checklist-backend","packageId":id}})).unwrap(),
            serde_json::from_value(json!({"type":"insert","rootId":"desktop-ui","entry":{"id":"checklist-ui","packageId":id,"inject":[maka_assistant::todo::client_service(id)]}})).unwrap(),
        ])].into(),
        ..Default::default()
    }
}
async fn watch(peer: &mut Peer, session: &str) -> (Value, Value) {
    let clients = success(
        peer.rpc("plugin.client.query", json!({"kind":"snapshot"}))
            .await,
    );
    let entry = clients["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == "z.checklist")
        .unwrap();
    let binding = json!({"method":"watch","sessionId":session,"client":{
        "entryId":entry["entryId"],"extensionId":entry["extensionId"],"activation":entry["activation"],"contentDigest":entry["contentDigest"],"clientDigest":entry["clientDigest"]
    }});
    let target = success(
        peer.rpc("plugin.remote", json!({"kind":"bind","binding":binding}))
            .await,
    )["target"]
        .clone();
    let document = success(
        peer.rpc("plugin.remote", json!({"kind":"open_document"}))
            .await,
    )["document"]
        .clone();
    let stream = success(peer.rpc("plugin.remote", json!({"kind":"open","document":document,"binding":binding,"target":target,"input":null})).await)["stream"].clone();
    (document, stream)
}
async fn snapshot(peer: &mut Peer, document: &Value, stream: &Value) -> Vec<Value> {
    let mut items = Vec::new();
    let mut revision = None;
    loop {
        let next = success(
            peer.rpc(
                "plugin.remote",
                json!({"kind":"next","document":document,"stream":stream}),
            )
            .await,
        );
        if next["kind"] == "pending" {
            continue;
        }
        let page = &next["item"];
        assert_eq!(page["offset"], items.len());
        assert!(serde_json::to_vec(page).unwrap().len() <= 64 * 1024);
        if let Some(revision) = &revision {
            assert_eq!(&page["revision"], revision);
        } else {
            revision = Some(page["revision"].clone());
        }
        items.extend(page["items"].as_array().unwrap().iter().cloned());
        if page["total"] == items.len() {
            return items;
        }
    }
}
fn success(value: Value) -> Value {
    assert_eq!(value["ok"], true, "{value}");
    value["result"].clone()
}
