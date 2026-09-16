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

use maka_runtime_host::server::{Host, HostError, websocket::WebSocketListener};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub(super) async fn run(
    path: &Path,
    websocket: Option<std::net::SocketAddr>,
    expected_root: Option<&str>,
) -> Result<(), maka_runtime_host::server::HostError> {
    use maka_event_log::root::{ROOT_MARKER, RootNamespaces, RootOwner};
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
            lifecycle_mode: maka_runtime_host::server::LifecycleMode::Service,
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
    let result = listen(endpoint, websocket, host, cancellation).await;
    result.and(registration.remove())
}

/// Process policy belongs to the CLI, never to an embedded Host library.
pub(super) async fn listen(
    endpoint: super::endpoint::LocalEndpoint,
    websocket: Option<WebSocketListener>,
    host: Arc<Host>,
    cancellation: CancellationToken,
) -> Result<(), HostError> {
    let _finished = cancellation.clone().drop_guard();
    watch_shutdown(
        host.wait_for_drain(),
        cancellation.clone(),
        Duration::from_secs(10),
    );
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

fn watch_shutdown(
    draining: impl Future<Output = ()> + Send + 'static,
    cancellation: CancellationToken,
    grace: Duration,
) {
    let runtime = tokio::runtime::Handle::current();
    // Observe before polling the listener: its synchronous diagnostics can block.
    // Neither notification needs a Tokio worker or I/O driver to make progress.
    // The thread owns no Host/Root and stays armed through runtime teardown;
    // normal process exit needs no join. Never block it on diagnostic output.
    if std::thread::Builder::new()
        .name("host-exit".into())
        .spawn(move || {
            runtime.block_on(async {
                tokio::select! {
                    _ = draining => {},
                    _ = cancellation.cancelled() => {},
                }
            });
            std::thread::sleep(grace);
            // Recovery, not this exit, determines outcomes of interrupted work.
            std::process::exit(70);
        })
        .is_err()
    {
        std::process::exit(70);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        time::Instant,
    };

    #[test]
    fn shutdown_deadline_bounds_blocking_runtime_teardown_without_delaying_normal_exit() {
        const CHILD: &str = "MAKA_SHUTDOWN_DEADLINE_TEST";
        if let Ok(mode) = std::env::var(CHILD) {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let draining = CancellationToken::new();
                let finished = CancellationToken::new();
                let _finished = finished.clone().drop_guard();
                watch_shutdown(
                    draining.clone().cancelled_owned(),
                    finished,
                    Duration::from_millis(250),
                );
                if mode == "synchronous" {
                    draining.cancel();
                    // No yield between announcing drain and blocking the listener.
                    std::thread::sleep(Duration::from_secs(60));
                } else if mode == "teardown" {
                    let (started, ready) = tokio::sync::oneshot::channel();
                    tokio::task::spawn_blocking(move || {
                        started.send(()).unwrap();
                        std::thread::sleep(Duration::from_secs(60));
                    });
                    ready.await.unwrap();
                }
                // A direct listener return must also bound final runtime teardown.
            });
            drop(runtime);
            return;
        }
        for (mode, code) in [("normal", 0), ("synchronous", 70), ("teardown", 70)] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "serve::tests::shutdown_deadline_bounds_blocking_runtime_teardown_without_delaying_normal_exit"])
                .env(CHILD, mode)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("Host exit deadline did not terminate blocked cleanup");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(status.code(), Some(code));
        }
    }
}
