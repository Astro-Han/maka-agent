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

use super::super::support::{client_probe::ClientFixture, peer::Peer};
use maka_client::{Client, Notification};
use maka_runtime_host::server::{Host, HostError, local::LocalListener};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) struct Running {
    pub client: Client,
    pub notices: tokio::sync::mpsc::Receiver<Notification>,
    stop: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), HostError>>,
}
impl Running {
    pub async fn open(fixture: &ClientFixture) -> Self {
        let owner = fixture.owner();
        let root = owner.root_id().to_owned();
        let host = Host::open(owner).await.unwrap();
        let (peer, hello) = Peer::handshake(host.clone(), "bundle-bootstrap").await;
        peer.close().await;
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("h.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-bundle-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host, stop.clone()),
        );
        let (client, notices) = Client::connect(
            maka_client::local::open_stream(&endpoint).await.unwrap(),
            &root,
            hello["hostEpoch"].as_str().unwrap(),
            maka_client::Operations,
        )
        .await
        .unwrap();
        Self {
            client,
            notices,
            stop,
            server,
        }
    }
    pub async fn close(mut self) {
        self.client.disconnect();
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(10), &mut self.server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.client.disconnect();
        self.stop.cancel();
    }
}
