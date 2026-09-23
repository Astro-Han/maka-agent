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
    support::{client_probe::ClientFixture, peer::Peer},
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// A real consumer Entry must receive the public callable contribution through injection.
struct Consumer(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl maka_plugins::kernel::Plugin for Consumer {
    fn activate(
        &self,
        context: maka_plugins::kernel::PluginContext,
        _: Value,
    ) -> futures_util::future::BoxFuture<'static, Result<maka_plugins::contributions::Staged, String>>
    {
        let calls = self.0.clone();
        Box::pin(async move {
            let method = context
                .services
                .method(maka_jev::decision::SERVICE)
                .map_err(|e| e.to_string())?
                .ok_or("Jev was not injected")?;
            let result = method
                .call::<_, Value>(
                    json!({"state":{},"questions":{
                        "ready":{"type":"noul","instructions":"Is the service ready?"}
                    }}),
                    None,
                    CancellationToken::new(),
                )
                .await;
            if !matches!(
                result,
                Err(maka_plugins::services::method::Error::Invalid(_))
            ) {
                return Err("Jev did not enforce caller admission".into());
            }
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(maka_plugins::contributions::Staged::default())
        })
    }
}
fn consumer_setup(
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> maka_runtime_host::plugins::Setup {
    use std::{collections::BTreeMap, sync::Arc};
    maka_runtime_host::plugins::Setup {
        builtins: BTreeMap::from([("example.jev-consumer".into(), Arc::new(maka_plugins::kernel::Definition {
            id: "example.jev-consumer".into(), revision: "1".into(), dependencies: vec![],
            inject: vec![maka_jev::decision::SERVICE.into()], plugin: Arc::new(Consumer(calls)),
        }))]),
        layers: BTreeMap::from([("example.jev-consumer".into(), serde_json::from_value(json!([
            {"type":"insert","rootId":"profile","entry":{"id":"example.jev-consumer","packageId":"example.jev-consumer"}}
        ])).unwrap())]),
        ..Default::default()
    }
}

async fn binding(peer: &mut Peer) -> Value {
    let snapshot = peer
        .rpc("plugin.client.query", json!({"kind":"snapshot"}))
        .await;
    let entry = snapshot["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == "maka.jev")
        .unwrap();
    json!({"client":{
        "entryId":entry["entryId"],"extensionId":entry["extensionId"],
        "activation":entry["activation"],"contentDigest":entry["contentDigest"],
        "clientDigest":entry["clientDigest"]
    },"method":"request"})
}

async fn open(peer: &mut Peer) -> Value {
    let binding = binding(peer).await;
    let bound = peer
        .rpc("plugin.remote", json!({"kind":"bind","binding":binding}))
        .await;
    assert_eq!(bound["ok"], true, "{bound}");
    let opened = peer
        .rpc("plugin.remote", json!({"kind":"open_document"}))
        .await;
    assert_eq!(opened["ok"], true, "{opened}");
    json!({"kind":"call","document":opened["result"]["document"],
        "binding":binding,"target":bound["result"]["target"]})
}
async fn call(peer: &mut Peer, envelope: &Value, input: Value) -> Value {
    let mut call = envelope.clone();
    call["input"] = input;
    peer.rpc("plugin.remote", call).await
}
async fn close(peer: &mut Peer, envelope: &Value) {
    let response = peer
        .rpc(
            "plugin.remote",
            json!({
                "kind":"close_document","document":envelope["document"]
            }),
        )
        .await;
    assert_eq!(response["ok"], true, "{response}");
}
async fn disable(peer: &mut Peer, disabled: bool) {
    let result = peer
        .rpc(
            "plugin.composition.apply",
            json!({"operations":[
                {"type":"update","entryId":"maka.jev","patch":{"disabled":disabled}}
            ]}),
        )
        .await;
    assert_eq!(result["ok"], true, "{result}");
    if !disabled {
        ready(peer).await;
    } else {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let clients = peer
                    .rpc("plugin.client.query", json!({"kind":"snapshot"}))
                    .await;
                if !clients["result"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["extensionId"] == "maka.jev")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoint_secrets_are_private_revisioned_and_survive_restart() {
    let fixture = ClientFixture::new("maka-jev-plugin-");
    let mut revision = Value::Null;
    let mut credential_revision = Value::Null;
    let url = "https://jev.example/custom/systemone";
    for reopened in [false, true] {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let host = Host::open_with_options(
            fixture.owner(),
            None,
            maka_runtime_host::server::HostOptions {
                plugins: consumer_setup(calls.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("jev.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-jev-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop),
        );
        let mut peer = Peer::new(host, "jev").await;
        ready(&mut peer).await;
        assert!(calls.load(std::sync::atomic::Ordering::SeqCst) > 0);
        let envelope = open(&mut peer).await;
        let read = call(&mut peer, &envelope, json!({"kind":"read"})).await;
        assert_eq!(read["ok"], true, "{read}");
        let snapshot = &read["result"]["value"]["snapshot"];
        if reopened {
            assert_eq!(snapshot["revision"], revision);
            assert_eq!(snapshot["credentialRevision"], credential_revision);
            assert_eq!(snapshot["settings"]["url"], url);
            assert_eq!(snapshot["configured"], true);
            assert_eq!(snapshot["headerNames"], json!(["X-Secret"]));
            assert!(!read.to_string().contains("private-test"));
            let changed = call(
                &mut peer,
                &envelope,
                json!({"kind":"configure",
                "expectedRevision":revision,"settings":{"enabled":true,
                "url":"https://other.example/jev", "model":"custom", "timeoutMs":1000}}),
            )
            .await;
            assert_eq!(changed["ok"], true, "{changed}");
            assert_eq!(changed["result"]["value"]["snapshot"]["configured"], false);
            assert_eq!(
                changed["result"]["value"]["snapshot"]["headerNames"],
                json!([])
            );
        } else {
            assert_eq!(snapshot["settings"]["enabled"], false);
            let mutation = json!({"kind":"configure","expectedRevision":null,
                "settings":{"enabled":true,"url":url,"model":"custom","timeoutMs":1000}});
            let saved = call(&mut peer, &envelope, mutation.clone()).await;
            assert_eq!(saved["ok"], true, "{saved}");
            revision = saved["result"]["value"]["snapshot"]["revision"].clone();
            assert!(revision.is_u64());
            let conflict = call(&mut peer, &envelope, mutation).await;
            assert_eq!(conflict["ok"], false, "{conflict}");
            let credential = json!({"kind":"credential","url":url,"expectedRevision":null,
                "secret":{"apiKey":"private-test-key","headers":{"X-Secret":"private-test-header"}}});
            let saved = call(&mut peer, &envelope, credential.clone()).await;
            assert_eq!(saved["ok"], true, "{saved}");
            assert!(!saved.to_string().contains("private-test"));
            credential_revision =
                saved["result"]["value"]["snapshot"]["credentialRevision"].clone();
            assert!(credential_revision.is_u64());
            let conflict = call(&mut peer, &envelope, credential).await;
            assert_eq!(conflict["ok"], false, "{conflict}");
            disable(&mut peer, true).await;
            let retired = call(&mut peer, &envelope, json!({"kind":"read"})).await;
            assert_eq!(retired["ok"], false, "{retired}");
            disable(&mut peer, false).await;
        }
        close(&mut peer, &envelope).await;
        peer.close().await;
        drop(cleanup);
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
