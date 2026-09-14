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

pub use crate::support::peer::Peer;
use maka_config::ConfigurationStore;
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub ns: RootNamespaces,
    pub endpoint: std::path::PathBuf,
}
impl Fixture {
    pub async fn new(proxy_port: Option<u16>) -> Self {
        #[cfg(unix)]
        let temp = {
            use std::os::unix::fs::PermissionsExt;
            tempfile::Builder::new()
                .prefix("maka-oauth-")
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in("/tmp")
                .unwrap()
        };
        #[cfg(windows)]
        let temp = tempfile::tempdir().unwrap();
        let ns = RootNamespaces {
            ownership: temp.path().join("owners"),
            control: temp.path().join("control"),
        };
        let owner = Arc::new(RootOwner::create(&temp.path().join("root"), &ns).unwrap());
        let store = ConfigurationStore::for_root(owner).await.unwrap();
        if let Some(port) = proxy_port {
            let policy = store.runtime_policy().await.unwrap();
            let mut proxy = policy.policy.network_proxy;
            proxy.enabled = true;
            proxy.host = "127.0.0.1".into();
            proxy.port = port;
            store
                .set_network_proxy(policy.revision, proxy)
                .await
                .unwrap();
        }
        store.close().await.unwrap();
        #[cfg(unix)]
        let endpoint = temp.path().join("host.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-oauth-{}", uuid::Uuid::new_v4()));
        Self { temp, ns, endpoint }
    }
    pub fn owner(&self) -> RootOwner {
        RootOwner::open(&self.temp.path().join("root"), &self.ns).unwrap()
    }
    pub async fn serve(&self) -> (Arc<Host>, CancellationToken, tokio::task::JoinHandle<()>) {
        let host = Host::open(self.owner()).await.unwrap();
        let listener = LocalListener::bind(&self.endpoint).unwrap();
        let cancel = CancellationToken::new();
        let server = tokio::spawn({
            let (host, cancel) = (host.clone(), cancel.clone());
            async move {
                listener.serve(host, cancel).await.unwrap();
            }
        });
        (host, cancel, server)
    }
}
impl Peer {
    pub async fn publish(&mut self, id: &str) {
        let value = self
            .rpc(
                "client.capability.replace",
                json!({
                    "registrationId":id, "offers":[],
                    "services":[{"serviceId":"oauth_presentation","version":"1"}]
                }),
            )
            .await;
        assert_eq!(value["ok"], true, "{value}");
    }
}
pub fn start(id: &str) -> Value {
    json!({"attemptId":id,"target":{"kind":"create","providerType":"xai-oauth"}})
}
pub async fn finish(task: tokio::task::JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}
