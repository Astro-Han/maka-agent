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

use super::{activation, directory, store};
use maka_client::{Client, Notification, local};
use maka_event_log::root::{self, FileLease, RootNamespaces, RootOwner};
use maka_process::detached::{self, Child};
use maka_runtime_host::server::HostError;
use sha2::{Digest, Sha256};
use std::{io, path::PathBuf, sync::Arc, time::Duration};
use tokio::{io::AsyncWriteExt, sync::mpsc};

type Connection = (Client, mpsc::Receiver<Notification>);

/// Interactive startup does not install, update, repair or take over a Host.
/// The UI calls this only after terminal validation, in a cancellable background job.
pub async fn connect(path: PathBuf) -> Result<Connection, HostError> {
    crate::operation::without_progress(connect_inner(path)).await
}

async fn connect_inner(path: PathBuf) -> Result<Connection, HostError> {
    let mut child = None;
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let (_launch, root_id) = loop {
            let root_path = path.clone();
            let result = tokio::task::spawn_blocking(move || {
                let namespaces = RootNamespaces::for_current_account()?;
                std::fs::create_dir_all(&root_path)?;
                let canonical = root_path.canonicalize()?;
                let key = format!(
                    "{:x}",
                    Sha256::digest(canonical.as_os_str().as_encoded_bytes())
                );
                root::private_directory(&namespaces.ownership)?;
                // Before initialization there is no Root ID. Serialize launchers
                // for this path through Ready, including partial marker/database
                // publication; the Root writer lease remains the sole authority.
                let launch = FileLease::acquire(
                    &namespaces.ownership.join(format!("tui-launch-{key}.lock")),
                )?;
                let id = root::initialize(&root_path, &namespaces)?;
                launch.validate()?;
                Ok::<_, io::Error>((launch, id))
            })
            .await?;
            match result {
                Ok(initialized) => break initialized,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => pause().await,
                Err(error) => return Err(error.into()),
            }
        };
        if let Some(connection) = live(&path, &root_id).await? {
            return Ok(connection);
        }
        let directory = directory(&root_id)?;
        root::private_directory(&directory)?;
        let lease = loop {
            match FileLease::acquire(&directory.join("executor.lock")) {
                Ok(lease) => break Arc::new(lease),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if let Some(connection) = live(&path, &root_id).await? {
                        return Ok(connection);
                    }
                    pause().await;
                }
                Err(error) => return Err(error.into()),
            }
        };
        lease.validate()?;
        if let Some(connection) = live(&path, &root_id).await? {
            return Ok(connection);
        }
        match store::read(&directory).await? {
            store::Installation::Installed(deployment) => {
                let root = root::resolve(&path)?;
                deployment.validate(&root, &directory)?;
                deployment.require_active()?;
                // Keep its probe resident until the actual interactive handshake.
                let (probe, _) = activation::connect_or_launch(&deployment, lease.clone()).await?;
                let connection = live(&path, &root_id)
                    .await?
                    .ok_or("Activated Host is unavailable")?;
                lease.validate()?;
                drop(probe);
                return Ok(connection);
            }
            store::Installation::Incomplete => {
                return Err("Host deployment installation is incomplete".into());
            }
            store::Installation::Missing => {}
        }
        loop {
            lease.validate()?;
            if let Some(connection) = live(&path, &root_id).await? {
                if let Some(child) = &mut child {
                    release(child).await?;
                }
                return Ok(connection);
            }
            if let Some(child) = &mut child {
                if let Some(status) = child.try_wait()? {
                    return Err(format!("Host candidate exited before Ready: {status}").into());
                }
            } else {
                match RootOwner::open(&path, &RootNamespaces::for_current_account()?) {
                    Ok(owner) => {
                        if owner.root_id() != root_id {
                            return Err("State Root changed before launch".into());
                        }
                        let canonical = owner.canonical_path().to_owned();
                        drop(owner);
                        child = Some(
                            detached::spawn(
                                &std::env::current_exe()?,
                                &[
                                    "host",
                                    "candidate",
                                    "--root",
                                    canonical.to_str().ok_or("State Root must be UTF-8")?,
                                    "--expected-root-id",
                                    &root_id,
                                    "--startup-attempt-id",
                                    &uuid::Uuid::new_v4().to_string(),
                                    "--owner-stdin",
                                    "--initial-connection-timeout-ms",
                                    "30000",
                                ],
                            )
                            .await?,
                        );
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
            pause().await;
        }
    })
    .await
    .map_err(|_| HostError::from("Host did not become Ready before the startup deadline"))
    .and_then(|result| result);
    if result.is_err()
        && let Some(mut child) = child
        && let Err(cleanup) =
            activation::stop_failed_candidate(&mut child, Duration::from_secs(5)).await
    {
        return Err(format!("{}; candidate cleanup: {cleanup}", result.err().unwrap()).into());
    }
    result
}

async fn pause() {
    tokio::time::sleep(Duration::from_millis(25)).await;
}

async fn release(child: &mut Child) -> Result<(), HostError> {
    child
        .stdin
        .take()
        .ok_or("candidate owner pipe is missing")?
        .write_all(b"{\"kind\":\"runtime-host-launch-owner-release\"}\n")
        .await?;
    Ok(())
}

async fn live(
    path: &std::path::Path,
    expected_root: &str,
) -> Result<Option<Connection>, HostError> {
    // A candidate has not published discovery until database initialization is
    // complete. Do not inspect its transient SQLite startup files as a live root.
    if !RootNamespaces::for_current_account()?
        .control
        .join(expected_root)
        .join("registration.json")
        .try_exists()?
    {
        return Ok(None);
    }
    let path = path.to_owned();
    let discovery = match tokio::task::spawn_blocking(move || local::read_discovery(&path)).await? {
        Ok(discovery) => discovery,
        Err(error) if absent(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    if discovery.root_id != expected_root {
        return Err("State Root changed during startup".into());
    }
    let stream = match local::open_stream(&discovery.endpoint).await {
        Ok(stream) => stream,
        Err(error) if absent(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(
        Client::connect(
            stream,
            expected_root,
            &discovery.host_epoch,
            maka_client::Operations,
        )
        .await?,
    ))
}

fn absent(error: &HostError) -> bool {
    error.downcast_ref::<io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        )
    })
}
