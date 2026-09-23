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

use gpui_kit::Global;
use maka_client::{Client, Notification};
use std::{future::Future, path::PathBuf, time::Duration};
use tokio::sync::mpsc;

/// `maka-client` needs a Tokio reactor; GPUI does not provide one. Every
/// client call runs here and the UI awaits only the runtime-agnostic handle.
pub struct Host {
    runtime: tokio::runtime::Runtime,
}

impl Global for Host {}

impl Host {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("maka-host-client")
                .enable_all()
                .build()?,
        })
    }

    pub fn spawn<F>(&self, future: F) -> impl Future<Output = Result<F::Output, String>> + use<F>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let task = self.runtime.spawn(future);
        async move { task.await.map_err(|error| error.to_string()) }
    }
}

pub async fn connect(root: PathBuf) -> Result<(Client, mpsc::Receiver<Notification>), String> {
    let attempt = async {
        let discovery =
            tokio::task::spawn_blocking(move || maka_client::local::read_discovery(&root))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        let stream = maka_client::local::open_stream(&discovery.endpoint)
            .await
            .map_err(|error| error.to_string())?;
        Client::connect(
            stream,
            &discovery.root_id,
            &discovery.host_epoch,
            maka_client::Operations,
        )
        .await
        .map_err(|error| error.to_string())
    };
    tokio::time::timeout(Duration::from_secs(6), attempt)
        .await
        .map_err(|_| "Timed out connecting to Host".to_string())?
}
