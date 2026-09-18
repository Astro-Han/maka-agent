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

mod proxy;

use super::{
    javascript_plugins::{package, ready},
    support::{client_probe::ClientFixture, peer::Peer},
};
use maka_config::ConfigurationStore;
use maka_plugins::{composition::Scope, fiber::Fiber};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::json;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn http_streams_follow_proxy_permissions_and_invocation_settlement() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let fixture = ClientFixture::new("maka-http-plugin-");
        let proxy = proxy::Proxy::start().await;
        let configuration = ConfigurationStore::for_root(Arc::new(fixture.owner())).await.unwrap();
        let policy = configuration.runtime_policy().await.unwrap();
        let mut network = policy.policy.network_proxy;
        network.enabled = true;
        network.host = "127.0.0.1".into();
        network.port = proxy.port;
        network.bypass_list.clear();
        network.auto_bypass_domains.clear();
        configuration.set_network_proxy(policy.revision, network).await.unwrap();
        configuration.close().await.unwrap();
        let source = package(&fixture.workspace, "example.http", "shared", include_str!("../fixtures/http-plugin.mjs"), false);
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("http.sock");
        #[cfg(windows)]
        let endpoint = std::path::PathBuf::from(format!(r"\\.\pipe\maka-http-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(LocalListener::bind(&endpoint).unwrap().serve(host.clone(), stop.clone()));
        let mut peer = Peer::new(host.clone(), "plugin-http").await;
        ready(&mut peer).await;
        let installed = peer.rpc("plugin.package.install", json!({"sourcePath":source})).await;
        assert_eq!(installed["ok"], true, "{installed}");
        ready(&mut peer).await;
        for mode in ["ask", "bypass"] {
            let result = peer.rpc("session.create", json!({
                "sessionId":mode, "workspace":{"kind":"host_path","path":fixture.workspace},
                "executorId":"example.http", "permissionMode":mode
            })).await;
            assert_eq!(result["ok"], true, "{result}");
        }
        let inspector = Fiber::new("example.http", "inspector", Scope::Profile).unwrap();
        inspector.begin_loading().unwrap();
        let storage = host.plugin_storage(inspector.context()).unwrap();
        for (index, (session, command)) in [("ask", "denied"), ("bypass", "complete"), ("bypass", "complete"), ("bypass", "retire")].into_iter().enumerate() {
            let turn = format!("http-{index}");
            let started = peer.rpc("turn.start", json!({"sessionId":session,"turnId":turn,"content":{"text":command}})).await;
            assert_eq!(started["ok"], true, "{started}");
            if command == "retire" {
                while storage.read("waiting".into()).await.unwrap().is_none() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let disabled = peer.rpc("plugin.composition.apply", json!({
                    "operations":[{"type":"update","entryId":"example.http","patch":{"disabled":true}}]
                })).await;
                assert_eq!(disabled["ok"], true, "{disabled}");
            }
            loop {
                let state = peer.rpc("turn.query", json!({"sessionId":session,"turnId":turn})).await;
                assert_eq!(state["ok"], true, "{state}");
                if !matches!(state["result"]["status"].as_str(), Some("admitted" | "created" | "running")) {
                    assert_eq!(state["result"]["status"], if command == "retire" { "cancelled" } else { "completed" }, "{state}");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if index == 0 { assert_eq!(proxy.requests.load(Ordering::SeqCst), 0); }
            else {
                while proxy.closed.load(Ordering::SeqCst) < index {
                    tokio::task::yield_now().await;
                }
                assert_eq!(proxy.requests.load(Ordering::SeqCst), index * 5);
            }
        }
        ready(&mut peer).await;
        inspector.shutdown(tokio::time::Instant::now() + Duration::from_secs(1)).await.unwrap();
        peer.close().await;
        stop.cancel();
        server.await.unwrap().unwrap();
        cleanup.disarm();
    }).await.expect("HTTP resource settlement must make bounded progress");
}
