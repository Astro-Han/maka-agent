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

use super::{Installation, guards::Guards, policy, store};
use maka_process::bootstrap::{self, Preparation};
use maka_sandbox::{Network, filesystem::Policy, windows::WriteCapability};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Captured Host installation and trusted bootstrap binary. Plugins receive
/// authorized process capabilities, never this provisioning authority.
#[derive(Clone)]
pub struct Backend {
    state_root: PathBuf,
    helper: PathBuf,
}
impl Backend {
    pub fn new(state_root: &Path, helper: &Path) -> Self {
        Self {
            state_root: state_root.to_owned(),
            helper: helper.to_owned(),
        }
    }
}
impl bootstrap::Backend for Backend {
    fn prepare(
        &self,
        filesystem: Policy,
        network: Network,
        route: maka_network::Policy,
        executable: PathBuf,
        cwd: PathBuf,
    ) -> Preparation {
        let backend = self.clone();
        Box::pin(async move {
            let root = backend.state_root.clone();
            let helper = backend.helper.clone();
            let account_network = network.clone();
            let (execution, guards) = tokio::task::spawn_blocking(move || {
                let installation = Installation::new(&root);
                let _lease = store::shared_lease(&installation.root.join("lifecycle.lock"))
                    .map_err(|error| {
                        if error.kind() == io::ErrorKind::NotFound {
                            super::setup_required()
                        } else {
                            error
                        }
                    })?;
                if installation.status()? != super::Status::Ready {
                    return Err(super::setup_required());
                }
                let filesystem = filesystem
                    .compile()
                    .and_then(|policy| policy.process_snapshot())
                    .map_err(io::Error::other)?;
                let compiled = filesystem.compile().map_err(io::Error::other)?;
                let guards = Guards::prepare(&compiled)
                    .map_err(|error| context("protect filesystem boundaries", error))?;
                let result = (|| {
                    let plan = policy::Plan::compile(&filesystem, &helper, &executable, &cwd)
                        .map_err(|error| context("compile filesystem access", error))?;
                    let execution = installation
                        .execution(account_network, &plan.reads, &plan.writes)
                        .map_err(|error| context("prepare execution account", error))?;
                    guards.bind(execution.id)?;
                    Ok::<_, io::Error>(execution)
                })();
                match result {
                    Ok(execution) => Ok((execution, guards)),
                    Err(error) => match guards.finish() {
                        Ok(()) => Err(error),
                        Err(cleanup) => Err(io::Error::other(format!(
                            "{error}; protection cleanup pending: {cleanup}"
                        ))),
                    },
                }
            })
            .await
            .map_err(io::Error::other)??;
            let id = execution.id;
            let capability = execution.capability;
            let mut guards = Some(guards);
            let result = async {
                let proxy_address = execution.proxy_port.map(|port| {
                    std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port.get()))
                });
                let proxy = match execution.proxy_port {
                    Some(port) => Some(super::network::acquire(port, network, route).await?),
                    None => None,
                };
                let participants = vec![
                    execution.account.sid().to_owned(),
                    WriteCapability::new(capability).sid().to_owned(),
                ];
                let desktop = Arc::new(
                    bootstrap::desktop(
                        &backend.helper,
                        &execution.account,
                        &execution.password,
                        &participants,
                    )
                    .await
                    .map_err(|error| context("create private desktop", error))?,
                );
                let endpoint = bootstrap::Endpoint::new(&execution.account)
                    .map_err(|error| context("create runner channel", error))?;
                let runner = bootstrap::Identity {
                    account: execution.account,
                    password: execution.password,
                    job: execution.job,
                    leases: execution.leases,
                    desktop,
                }
                .start(backend.helper.clone(), endpoint.id())
                .await?;
                let root = backend.state_root.clone();
                let read_helper = backend.helper.clone();
                let guards = guards.take().expect("one execution owner");
                Ok::<_, io::Error>(bootstrap::Launch {
                    runner: runner.with_proxy(proxy).with_cleanup(move || {
                        Installation::new(&root).settle(id)?;
                        guards.finish()?;
                        super::reads::start(&root, &read_helper);
                        Ok(())
                    }),
                    endpoint,
                    capabilities: vec![capability],
                    proxy_address,
                })
            }
            .await;
            match result {
                Ok(launch) => {
                    super::reads::start(&backend.state_root, &backend.helper);
                    Ok(launch)
                }
                Err(error) => {
                    let root = backend.state_root;
                    let cleanup = tokio::task::spawn_blocking(move || {
                        let settled = Installation::new(&root).settle(id);
                        let guards = guards
                            .expect("failed startup retains protection ownership")
                            .finish();
                        settled.and(guards)
                    })
                    .await
                    .map_err(io::Error::other)?;
                    match cleanup {
                        Ok(()) => Err(error),
                        Err(cleanup) => Err(io::Error::other(format!(
                            "{error}; sandbox cleanup pending: {cleanup}"
                        ))),
                    }
                }
            }
        })
    }
}

fn context(action: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("Windows sandbox could not {action}: {error}"),
    )
}
