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

use super::{Error, Executions};
use maka_plugins::{
    authorization::{Boundary, Capability},
    call::Scope,
    execution::SessionBoundary,
};
use maka_runtime::execution::SandboxMode;
use maka_sandbox::Destination;

impl Executions {
    pub(crate) async fn admit_plugin_network(
        &self,
        call: &Scope,
        destination: &Destination,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, Error> {
        let gate = tokio::select! {
            _ = call.cancellation.cancelled() => return Err(Error::Revoked),
            gate = self.interactions.own_admission() => gate,
        };
        self.check_plugin_network(call, destination).await?;
        Ok(gate)
    }

    pub(crate) async fn check_plugin_network(
        &self,
        call: &Scope,
        destination: &Destination,
    ) -> Result<(), Error> {
        if call.identity.agent().is_none() {
            return match self
                .plugin_resource_boundary(call, Capability::Network)
                .await?
            {
                Boundary::Session { .. } | Boundary::Workspace { .. } => Ok(()),
                Boundary::Profile | Boundary::Directory { .. } => Err(Error::Denied),
            };
        }
        let Boundary::Session { boundary, .. } = self.plugin_execution_boundary(call).await? else {
            return Err(Error::Denied);
        };
        if self
            .agent_network_allowed(call, &boundary, destination)
            .await?
        {
            Ok(())
        } else {
            Err(Error::Denied)
        }
    }

    async fn agent_network_allowed(
        &self,
        call: &Scope,
        boundary: &SessionBoundary,
        destination: &Destination,
    ) -> Result<bool, Error> {
        if boundary.sandbox_mode == SandboxMode::DangerFullAccess {
            return Ok(true);
        }
        let grants = self
            .plugin_permission_grants(call, boundary.boundary_revision)
            .await?;
        Ok(grants
            .iter()
            .any(|grant| grant.permissions.network.allows(destination)))
    }

    pub(crate) async fn plugin_network_policy(
        &self,
        call: &Scope,
        destination: &Destination,
    ) -> Result<maka_network::Policy, Error> {
        self.check_plugin_network(call, destination).await?;
        let network = self
            .configuration
            .network_configuration()
            .await
            .map_err(|error| Error::Host(error.to_string()))?;
        maka_network::Policy::from_settings(&network.proxy, network.password.as_deref())
            .map_err(|error| Error::Host(error.to_string()))
    }
}
