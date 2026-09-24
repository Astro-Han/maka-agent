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

use super::{directory, service, store};
use crate::host_client::HostClient;
use maka_event_log::root::{self, FileLease, RootNamespaces, RootOwner};
use maka_protocol::{
    Operation,
    host::{RetirementInput, RetirementResult, decode_retirement_result},
};
use maka_runtime_host::server::HostError;
use maka_tui::{ShutdownOutcome, ShutdownRequest};
use std::{io, sync::Arc, time::Duration};

pub async fn shutdown(request: ShutdownRequest) -> Result<ShutdownOutcome, HostError> {
    crate::operation::without_progress(async {
        tokio::time::timeout(Duration::from_secs(30), stop(request)).await?
    })
    .await
}

async fn stop(request: ShutdownRequest) -> Result<ShutdownOutcome, HostError> {
    let root = root::resolve(&request.root)?;
    if root.root_id() != request.identity.root_id {
        return Err("State Root changed before shutdown".into());
    }
    let directory = directory(root.root_id())?;
    root::private_directory(&directory)?;
    // Startup, service control and upgrades cannot race this shutdown.
    let lease = Arc::new(FileLease::acquire(&directory.join("executor.lock"))?);
    let deployment = match store::read(&directory).await? {
        store::Installation::Installed(deployment) => {
            deployment.validate(&root, &directory)?;
            Some(deployment)
        }
        store::Installation::Missing => None,
        store::Installation::Incomplete => return Err("Host deployment is incomplete".into()),
    };
    let namespaces = RootNamespaces::for_current_account()?;
    let owner = match RootOwner::open(&request.root, &namespaces) {
        Ok(owner) => owner,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            let generation = deployment.as_ref().map(|d| d.generation());
            let mut operator = HostClient::connect(&request.root, generation.as_deref()).await?;
            if operator.root_id() != request.identity.root_id {
                return Err("Host identity changed before shutdown".into());
            }
            // This is an exit, not an upgrade: do not arrange future task resumption.
            // The Host checks work/other clients and seals admission atomically.
            let input = RetirementInput {
                expected_host_epoch: request.identity.host_epoch.clone(),
                allow_interrupt_active_tasks: request.interrupt,
                allow_cooperative_handoff: Some(false),
                allow_idle_connections: Some(false),
                handoff_connection_id: Some(request.identity.connection_id.clone()),
            };
            let result = operator
                .request(Operation::HostUpgradePrepare, serde_json::to_value(input)?)
                .await?;
            if decode_retirement_result(&result)? == RetirementResult::ActiveTasks {
                return Ok(ShutdownOutcome::Busy);
            }
            drop(operator);
            // The receipt only admits shutdown. Root release proves owned work
            // and storage have drained; do not replay an uncertain stop request.
            loop {
                match RootOwner::open(&request.root, &namespaces) {
                    Ok(owner) => break owner,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if let Ok(live) = maka_client::local::read_discovery(&request.root)
                            && live.host_epoch != request.identity.host_epoch
                        {
                            return Err("Another Host started during shutdown".into());
                        }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Err(error) => return Err(error.into()),
    };
    if owner.root_id() != request.identity.root_id
        || owner.canonical_path() != root.canonical_path()
    {
        return Err("State Root changed during shutdown".into());
    }
    owner.validate_current()?;
    if let Some(deployment) = deployment {
        service::stop(deployment, lease.clone(), Arc::new(owner)).await?;
    }
    lease.validate()?;
    Ok(ShutdownOutcome::Stopped)
}
