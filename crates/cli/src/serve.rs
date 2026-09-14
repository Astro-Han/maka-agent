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

use serde_json::json;
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub(super) async fn run(
    path: &Path,
    websocket: Option<std::net::SocketAddr>,
) -> Result<(), maka_runtime_host::server::HostError> {
    use maka_event_log::root::{ROOT_MARKER, RootNamespaces, RootOwner};
    use maka_runtime_host::server::{Host, websocket::WebSocketListener};
    let websocket = match websocket {
        Some(address) => Some(WebSocketListener::bind(address, Vec::new()).await?),
        None => None,
    };
    let namespaces = RootNamespaces::for_current_account()?;
    let owner = if path.join(ROOT_MARKER).exists() {
        RootOwner::open(path, &namespaces)?
    } else {
        RootOwner::create(path, &namespaces)?
    };
    let host = Host::open_with_options(
        owner,
        global_instructions()?,
        maka_runtime_host::server::HostOptions {
            skill_home: home_directory()?,
            ..Default::default()
        },
    )
    .await?;
    let endpoint = super::endpoint::LocalEndpoint::bind()?;
    let cancellation = CancellationToken::new();
    let _signals = super::signals::watch(cancellation.clone())?;
    let mut ready = json!({"kind":"ready","rootId":host.root_id(),"socketPath":endpoint.path});
    if let Some(listener) = &websocket {
        ready["websocketUrl"] = format!("ws://{}/runtime-host", listener.local_addr()?).into();
    }
    println!("{ready}");
    match websocket {
        Some(websocket) => {
            endpoint
                .listener
                .serve_with_websocket(websocket, host, cancellation)
                .await
        }
        None => endpoint.listener.serve(host, cancellation).await,
    }
}

pub(super) fn global_instructions()
-> Result<Option<std::path::PathBuf>, maka_runtime_host::server::HostError> {
    Ok(home_directory()?.map(|home| home.join(".maka")))
}

pub(super) fn home_directory()
-> Result<Option<std::path::PathBuf>, maka_runtime_host::server::HostError> {
    #[cfg(unix)]
    let instructions = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(std::path::PathBuf::from);
    #[cfg(windows)]
    let instructions = Some(maka_event_log::root::windows::account_home()?);
    Ok(instructions)
}
