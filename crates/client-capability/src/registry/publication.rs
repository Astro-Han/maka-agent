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

use crate::{ContractId, Endpoint, Identity};
use maka_runtime::capability::{HostFrame, Manifest, Offer};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex, Weak},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(super) type Residency = Arc<Mutex<HashMap<String, Weak<Registration>>>>;

/// An immutable publication. Runs and invocations pin this exact allocation.
pub struct Registration {
    pub(super) provider_id: String,
    pub(super) connection_id: Uuid,
    pub(super) identity: Arc<Identity>,
    pub(super) manifest: Manifest,
    pub(super) contracts: BTreeMap<ContractId, usize>,
    pub(super) endpoint: Endpoint,
    pub(super) residency: Residency,
    pub(super) invocations: CancellationToken,
}

impl Registration {
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }
    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }
    pub fn identity(&self) -> &Arc<Identity> {
        &self.identity
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn offers(&self) -> impl Iterator<Item = (&ContractId, &Offer)> {
        self.contracts
            .iter()
            .map(|(id, index)| (id, &self.manifest.offers[*index]))
    }

    pub fn offer(&self, contract: &ContractId) -> Option<&Offer> {
        self.contracts
            .get(contract)
            .map(|index| &self.manifest.offers[*index])
    }

    pub fn available(&self) -> bool {
        !self.invocations.is_cancelled()
    }

    pub fn invocations(&self) -> CancellationToken {
        self.invocations.clone()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.manifest.session_id.as_deref()
    }

    pub fn visible_to(&self, session_id: Option<&str>) -> bool {
        self.session_id().is_none() || self.session_id() == session_id
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // Serialize final release with ID reuse. Weak::upgrade alone would
        // permit reuse while the old allocation's destructor is still running.
        let mut resident = self.residency.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self.endpoint.send(HostFrame::RegistrationRelease {
            registration_id: self.manifest.registration_id.clone(),
        });
        resident.remove(&self.manifest.registration_id);
    }
}

impl super::Registry {
    pub fn current(&self, provider_id: &str) -> Option<Arc<Registration>> {
        self.current_scoped(provider_id, None)
    }

    pub(super) fn current_scoped(
        &self,
        provider_id: &str,
        session_id: Option<&str>,
    ) -> Option<Arc<Registration>> {
        let provider = self.providers.get(provider_id)?;
        match session_id {
            Some(id) => provider.scoped.get(id).cloned(),
            None => provider.current.clone(),
        }
    }

    pub fn current_for_connection(&self, id: Uuid) -> Option<Arc<Registration>> {
        let connection = self.connections.get(&id)?;
        let registration = self.current(&connection.provider_id)?;
        (registration.connection_id == id && !connection.endpoint.closed().is_cancelled())
            .then_some(registration)
    }

    pub fn published(&self) -> impl Iterator<Item = &Arc<Registration>> {
        self.providers
            .values()
            .flat_map(|provider| provider.current.iter().chain(provider.scoped.values()))
    }
}
