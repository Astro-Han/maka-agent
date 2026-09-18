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

use super::Platform;
use maka_plugins::{
    client::Client,
    composition::Scope,
    contributions::Contribution,
    fiber::CallGuard,
    remote::{Endpoint, Target},
};
use maka_protocol::{OperationError, OperationErrorCode as Code, plugin::RemoteBinding};

pub(crate) struct Bound {
    pub client: Contribution<Client>,
    pub endpoint: Contribution<Endpoint>,
    pub target: Target,
}
pub(crate) struct Leases {
    _client: CallGuard,
    _endpoint: CallGuard,
}
impl Bound {
    pub fn admit(&self) -> Result<Leases, OperationError> {
        Ok(Leases {
            _client: self.client.admit().map_err(conflict)?,
            _endpoint: self.endpoint.admit().map_err(conflict)?,
        })
    }
}
impl Platform {
    /// Captures both UI and backend registrations. An already bound target may
    /// not resolve to a replacement handler, even within the same activation.
    pub(crate) fn bind_remote(
        &self,
        request: &RemoteBinding,
        expected: Option<&Target>,
    ) -> Result<Bound, OperationError> {
        let client = self
            .catalog
            .snapshot::<Client>(&Scope::DesktopUi)
            .entries
            .get(&request.client.entry_id)
            .cloned()
            .ok_or_else(|| conflict("Client entry is not effective"))?;
        let identity = client.owner.identity().map_err(conflict)?;
        let bundle = &client.value.bundle;
        if identity.activation != request.client.activation
            || identity.package_id != request.client.extension_id
            || bundle.content_digest != request.client.content_digest
            || bundle.client_digest != request.client.client_digest
        {
            return Err(conflict("Client activation or package bytes changed"));
        }
        let scope = request
            .session_id
            .as_ref()
            .map_or(Scope::Profile, |id| Scope::Session(id.clone()));
        let key =
            maka_plugins::remote::key(&identity.package_id, &request.method).map_err(conflict)?;
        let endpoint = self
            .catalog
            .snapshot::<Endpoint>(&scope)
            .entries
            .get(&key)
            .cloned()
            .ok_or_else(|| conflict("Remote handler is not effective"))?;
        let owner = endpoint.owner.identity().map_err(conflict)?;
        if owner.package_id != identity.package_id
            || endpoint.value.content_digest != bundle.content_digest
        {
            return Err(conflict("Client and Host package bytes do not match"));
        }
        let target = endpoint.value.target(&owner);
        if expected.is_some_and(|expected| expected != &target) {
            return Err(conflict("Remote backend registration changed"));
        }
        let bound = Bound {
            client,
            endpoint,
            target,
        };
        let _leases = bound.admit()?;
        Ok(bound)
    }
}
fn conflict(error: impl std::fmt::Display) -> OperationError {
    OperationError {
        code: Code::OperationConflict,
        message: error.to_string(),
    }
}
