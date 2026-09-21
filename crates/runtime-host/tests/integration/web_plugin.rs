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

async fn binding(peer: &mut Peer) -> Value {
    let snapshot = peer
        .rpc("plugin.client.query", json!({"kind":"snapshot"}))
        .await;
    let entry = snapshot["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == "maka.web")
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
                {"type":"update","entryId":"maka.web","patch":{"disabled":disabled}}
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
                    .any(|entry| entry["extensionId"] == "maka.web")
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
async fn remote_settings_are_revisioned_private_and_survive_retirement_and_restart() {
    let fixture = ClientFixture::new("maka-web-plugin-");
    let mut saved_revision = Value::Null;
    let mut credential_revision = Value::Null;
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("web.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-web-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop),
        );
        let mut peer = Peer::new(host, "web").await;
        if !reopened {
            ready(&mut peer).await;
        }
        if reopened {
            let clients = peer
                .rpc("plugin.client.query", json!({"kind":"snapshot"}))
                .await;
            assert!(
                !clients["result"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["extensionId"] == "maka.web")
            );
            disable(&mut peer, false).await;
        }
        let envelope = open(&mut peer).await;
        let snapshot = call(&mut peer, &envelope, json!({"kind":"read"})).await;
        assert_eq!(snapshot["ok"], true, "{snapshot}");
        let snapshot = &snapshot["result"]["value"]["snapshot"];
        if reopened {
            assert_eq!(snapshot["revision"], saved_revision);
            assert_eq!(
                snapshot["settings"],
                json!({"enabled":true,"source":"tavily"})
            );
            assert_eq!(snapshot["credential"]["configured"], true);
            assert_eq!(snapshot["credential"]["revision"], credential_revision);
            assert!(!snapshot.to_string().contains("private-test-key"));
            let deleted = call(
                &mut peer,
                &envelope,
                json!({"kind":"credential",
                "expectedRevision":credential_revision,"secret":null}),
            )
            .await;
            assert_eq!(deleted["ok"], true, "{deleted}");
            let cleared = call(&mut peer, &envelope, json!({"kind":"read"})).await;
            assert_eq!(
                cleared["result"]["value"]["snapshot"]["credential"]["configured"],
                false
            );
        } else {
            assert_eq!(snapshot["settings"]["enabled"], false);
            let mutation = json!({"kind":"configure","expectedRevision":null,
                "settings":{"enabled":true,"source":"tavily"}});
            let saved = call(&mut peer, &envelope, mutation.clone()).await;
            assert_eq!(saved["ok"], true, "{saved}");
            saved_revision = saved["result"]["value"]["revision"].clone();
            let conflict = call(&mut peer, &envelope, mutation).await;
            assert_eq!(
                conflict["ok"], false,
                "stale UI must not overwrite: {conflict}"
            );
            let credential = call(
                &mut peer,
                &envelope,
                json!({"kind":"credential",
                "expectedRevision":null,"secret":"private-test-key"}),
            )
            .await;
            assert_eq!(credential["ok"], true, "{credential}");
            credential_revision = credential["result"]["value"]["receipt"]["revision"].clone();
            assert!(credential_revision.is_u64(), "{credential}");
            disable(&mut peer, true).await;
            let retired = call(&mut peer, &envelope, json!({"kind":"read"})).await;
            assert_eq!(retired["ok"], false, "{retired}");
            let clients = peer
                .rpc("plugin.client.query", json!({"kind":"snapshot"}))
                .await;
            assert!(
                !clients["result"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["extensionId"] == "maka.web")
            );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an explicitly authorized TAVILY_API_KEY"]
async fn live_tavily_search_uses_plugin_credentials_and_remote_network_authority() {
    let secret = std::env::var("TAVILY_API_KEY").expect("explicit search credential required");
    let fixture = ClientFixture::new("maka-live-web-");
    let host = Host::open(fixture.owner()).await.unwrap();
    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("web.sock");
    #[cfg(windows)]
    let endpoint = std::path::PathBuf::from(format!(r"\\.\pipe\maka-web-{}", uuid::Uuid::new_v4()));
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop),
    );
    let mut peer = Peer::new(host, "live-web").await;
    ready(&mut peer).await;
    let envelope = open(&mut peer).await;
    let credential = call(
        &mut peer,
        &envelope,
        json!({"kind":"credential",
        "expectedRevision":null,"secret":secret}),
    )
    .await;
    assert_eq!(credential["ok"], true, "credential write failed");
    let saved = call(
        &mut peer,
        &envelope,
        json!({"kind":"configure",
        "expectedRevision":null,"settings":{"enabled":true,"source":"tavily"}}),
    )
    .await;
    assert_eq!(saved["ok"], true, "settings write failed");
    let result = call(
        &mut peer,
        &envelope,
        json!({"kind":"search",
        "operationId":uuid::Uuid::new_v4(),
        "query":{"query":"site:rust-lang.org current stable Rust release","limit":2}}),
    )
    .await;
    let deleted = call(
        &mut peer,
        &envelope,
        json!({"kind":"credential",
        "expectedRevision":credential["result"]["value"]["receipt"]["revision"],"secret":null}),
    )
    .await;
    close(&mut peer, &envelope).await;
    peer.close().await;
    drop(cleanup);
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(deleted["ok"], true, "credential cleanup failed");
    assert_eq!(result["ok"], true, "{result}");
    let results = &result["result"]["value"]["results"];
    let rows = results["rows"].as_array().unwrap();
    assert!(!rows.is_empty() && rows.len() <= 2);
    assert!(!result.to_string().contains(&secret));
    println!(
        "Tavily plugin Remote search: {} sources, truncated={}",
        rows.len(),
        results["truncated"]
    );
}
