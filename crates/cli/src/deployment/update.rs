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

use super::{Deployment, Mode, RootId, activation, directory, package, store, updates};
use crate::host_client::{HostClient, LiveHost};
use clap::Args;
use maka_event_log::root::{self, FileLease, RootNamespaces, RootOwner};
use maka_protocol::host::RetirementResult;
use maka_runtime_host::server::HostError;
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

#[derive(Args)]
pub(crate) struct Update {
    #[arg(long)]
    root_id: RootId,
    #[arg(long)]
    expected_deployment_id: Uuid,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=9_007_199_254_740_991))]
    expected_revision: u64,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Outcome {
    Ready {
        deployment: Deployment,
        host: LiveHost,
    },
    ActiveTasks {
        deployment: Deployment,
        target: Deployment,
    },
}

impl Update {
    pub async fn run(self, reconcile: bool) -> Result<(), HostError> {
        let directory = directory(&self.root_id.0)?;
        if !directory.is_dir() {
            return Err("Host deployment is not installed".into());
        }
        let lease = Arc::new(FileLease::acquire(&directory.join("executor.lock"))?);
        // Do not create a deployment database through update/reconcile.
        let store::Installation::Installed(current) = store::read(&directory).await? else {
            return Err("Host deployment is absent or incomplete".into());
        };
        let root = root::resolve(&current.root_path)?;
        current.validate(&root, &directory)?;
        if current.root_id != self.root_id.0 || current.deployment_id != self.expected_deployment_id
        {
            return Err("deployment identity or revision changed".into());
        }
        if current.mode != Mode::OnDemand {
            return Err("supervised updates require service coordination".into());
        }
        let (observed, pending) = updates::read(&directory, lease.clone()).await?;
        if observed != current
            || !(current.config_revision == self.expected_revision
                || (current.config_revision == self.expected_revision + 1 && pending.is_none()))
        {
            return Err("deployment revision changed".into());
        }
        let target = if reconcile {
            pending
        } else {
            let source = package::source(current.mode)?;
            let (executable, sha256) = if source == current.executable {
                (source, current.sha256.clone())
            } else {
                let stage_directory = directory.clone();
                let stage_lease = lease.clone();
                tokio::task::spawn_blocking(move || {
                    stage_lease.validate()?;
                    let package = package::stage(&stage_directory, &source)?;
                    stage_lease.validate()?;
                    Ok::<_, HostError>(package)
                })
                .await??
            };
            if sha256 == current.sha256 {
                if pending.is_some() {
                    return Err("another update is pending; reconcile it first".into());
                }
                None // Includes a lost acknowledgement after the same target committed.
            } else {
                if current.config_revision != self.expected_revision {
                    return Err("deployment changed before package staging".into());
                }
                let mut target = current.clone();
                target.config_revision += 1;
                target.executable = executable;
                target.sha256 = sha256;
                target.validate(&root, &directory)?;
                updates::prepare(&directory, lease.clone(), current.clone(), target.clone())
                    .await?;
                Some(target)
            }
        };
        let deployment = if let Some(target) = target {
            target.validate(&root, &directory)?;
            let Some(owner) = retire(&current).await? else {
                lease.validate()?;
                println!(
                    "{}",
                    serde_json::to_string(&Outcome::ActiveTasks {
                        deployment: current,
                        target
                    })?
                );
                return Ok(());
            };
            updates::commit(&directory, lease.clone(), owner, current, target.clone()).await?;
            target
        } else {
            current
        };
        // The executor is already held. Calling the public activate command here
        // would acquire it twice. A Ready failure never restores older code.
        let (client, host) = activation::connect_or_launch(&deployment).await?;
        lease.validate()?;
        println!(
            "{}",
            serde_json::to_string(&Outcome::Ready { deployment, host })?
        );
        drop(client);
        Ok(())
    }
}

/// A prepared receipt is not proof of release: only acquiring RootOwner is.
async fn retire(deployment: &Deployment) -> Result<Option<RootOwner>, HostError> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match RootOwner::open(
                &deployment.root_path,
                &RootNamespaces::for_current_account()?,
            ) {
                Ok(owner) => return Ok(Some(owner)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if let Ok(mut client) =
                HostClient::connect(&deployment.root_path, Some(&deployment.generation())).await
            {
                match client.retire(None, false).await? {
                    RetirementResult::ActiveTasks => return Ok(None),
                    RetirementResult::Prepared { .. } => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await?
}
