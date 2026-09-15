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
    expected_root: Option<&str>,
) -> Result<(), maka_runtime_host::server::HostError> {
    use maka_event_log::root::{ROOT_MARKER, RootNamespaces, RootOwner};
    use maka_runtime_host::server::{Host, websocket::WebSocketListener};
    let namespaces = RootNamespaces::for_current_account()?;
    let owner = if path.join(ROOT_MARKER).exists() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match RootOwner::open(path, &namespaces) {
                Ok(owner) => break owner,
                Err(error)
                    if expected_root.is_some()
                        && error.kind() == std::io::ErrorKind::WouldBlock
                        && tokio::time::Instant::now() < deadline =>
                {
                    // launchd can start the job while its registrar still
                    // holds Root. Admission happens only after acquisition.
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    } else if expected_root.is_none() {
        RootOwner::create(path, &namespaces)?
    } else {
        return Err("service State Root must already be installed".into());
    };
    if expected_root.is_some_and(|expected| expected != owner.root_id()) {
        return Err("service State Root identity changed".into());
    }
    let deployment = crate::deployment::admit(&owner, crate::deployment::Mode::Supervised).await?;
    if expected_root.is_some() && deployment.is_none() {
        return Err("service State Root must have a supervised deployment".into());
    }
    let websocket = match deployment
        .as_ref()
        .map(|deployment| deployment.websocket)
        .or(websocket)
    {
        Some(address) => Some(WebSocketListener::bind(address, Vec::new()).await?),
        None => None,
    };
    let host = Host::open_with_options(
        owner,
        global_instructions()?,
        maka_runtime_host::server::HostOptions {
            skill_home: home_directory()?,
            project_directory_roots: deployment
                .as_ref()
                .and_then(|deployment| deployment.project_directory_roots.clone()),
            generation: deployment
                .as_ref()
                .map(|deployment| deployment.generation()),
            ..Default::default()
        },
    )
    .await?;
    let endpoint = super::endpoint::LocalEndpoint::bind()?;
    let cancellation = CancellationToken::new();
    let _signals = super::signals::watch(cancellation.clone())?;
    let registration = host.publish_registration(
        &endpoint.path,
        websocket
            .as_ref()
            .map(|listener| listener.local_addr())
            .transpose()?,
    )?;
    let mut ready = json!({"kind":"ready","rootId":host.root_id(),"socketPath":endpoint.path});
    if let Some(listener) = &websocket {
        ready["websocketUrl"] = format!("ws://{}/runtime-host", listener.local_addr()?).into();
    }
    println!("{ready}");
    let result = match websocket {
        Some(websocket) => {
            endpoint
                .listener
                .serve_with_websocket(websocket, host, cancellation)
                .await
        }
        None => endpoint.listener.serve(host, cancellation).await,
    };
    result.and(registration.remove())
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
