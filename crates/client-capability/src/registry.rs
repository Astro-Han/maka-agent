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

use crate::{Endpoint, Error, Identity, PrincipalKind};
use maka_runtime::capability::{Affinity, HostPathAccess, Manifest, RegistrationResult};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;

mod bindings;
mod publication;
pub use bindings::{BindingError, BindingMode, PreparedBindings, Snapshot, SnapshotOffer};
pub use publication::Registration;
use publication::Residency;

struct Provider {
    identity: Arc<Identity>,
    residency: Residency,
    current: Option<Arc<Registration>>,
    active: Option<Uuid>,
}

struct Connection {
    provider_id: String,
    endpoint: Endpoint,
    superseded: bool,
}

/// Mutations are synchronous so composition can serialize registry and binding
/// changes under one admission gate without holding a lock across I/O.
#[derive(Default)]
pub struct Registry {
    providers: HashMap<String, Provider>,
    connections: HashMap<Uuid, Connection>,
    revision: u64,
    draining: bool,
    sessions: HashMap<String, bindings::SessionBindings>,
}

impl Registry {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn attach(
        &mut self,
        id: Uuid,
        identity: Identity,
        endpoint: Endpoint,
    ) -> Result<String, Error> {
        if self.draining {
            return Err(Error::Draining);
        }
        if self.connections.contains_key(&id) || endpoint.closed().is_cancelled() {
            return Err(Error::Invalid("connection is already attached or closed"));
        }
        if identity.capability_owner.is_some()
            && identity.principal_kind != PrincipalKind::CapabilityProvider
        {
            return Err(Error::Invalid(
                "only capability providers may declare an owner",
            ));
        }
        self.prune();
        let provider_id = identity.provider_id();
        if let Some(provider) = self.providers.get(&provider_id) {
            if *provider.identity != identity {
                return Err(Error::Invalid(
                    "provider authority changed across connections",
                ));
            }
        } else {
            self.providers.insert(
                provider_id.clone(),
                Provider {
                    identity: Arc::new(identity),
                    residency: Arc::default(),
                    current: None,
                    active: None,
                },
            );
        }
        self.connections.insert(
            id,
            Connection {
                provider_id: provider_id.clone(),
                endpoint,
                superseded: false,
            },
        );
        Ok(provider_id)
    }

    /// Input must first pass the protocol manifest decoder.
    pub fn replace(
        &mut self,
        connection_id: Uuid,
        manifest: Manifest,
    ) -> Result<RegistrationResult, Error> {
        if self.draining {
            return Err(Error::Draining);
        }
        let connection = self
            .connections
            .get(&connection_id)
            .ok_or(Error::Unavailable)?;
        if connection.superseded || connection.endpoint.closed().is_cancelled() {
            return Err(Error::Invalid("connection has been superseded or closed"));
        }
        let provider = self
            .providers
            .get_mut(&connection.provider_id)
            .expect("attached provider");
        if provider.identity.principal_kind == PrincipalKind::CapabilityProvider
            && (manifest.services.as_ref().is_some_and(|s| !s.is_empty())
                || manifest.offers.iter().any(|offer| {
                    offer.affinity != Affinity::Session
                        || offer.host_path_access != HostPathAccess::None
                }))
        {
            return Err(Error::Invalid(
                "trusted providers may publish only path-independent session tools",
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .filter(|r| *r <= 9_007_199_254_740_991)
            .ok_or(Error::Invalid("revision exhausted"))?;
        let mut proxy_names = HashSet::new();
        for tool in manifest.offers.iter().flat_map(|offer| &offer.tools) {
            if !proxy_names.insert(crate::proxy_tool_name(&tool.server_id, &tool.name)) {
                return Err(Error::Invalid("model proxy tool name collision"));
            }
        }
        let mut resident = provider.residency.lock().unwrap_or_else(|e| e.into_inner());
        if !resident.insert(manifest.registration_id.clone()) {
            return Err(Error::Invalid("registration identity is still resident"));
        }
        drop(resident);
        let registration = Arc::new(Registration {
            provider_id: connection.provider_id.clone(),
            connection_id,
            identity: provider.identity.clone(),
            contracts: manifest
                .offers
                .iter()
                .enumerate()
                .map(|(index, offer)| (crate::ContractId::of(offer), index))
                .collect(),
            manifest,
            endpoint: connection.endpoint.clone(),
            residency: provider.residency.clone(),
        });
        if let Some(old) = provider.active.filter(|old| *old != connection_id)
            && let Some(connection) = self.connections.get_mut(&old)
        {
            connection.superseded = true;
            connection.endpoint.supersede();
        }
        provider.active = Some(connection_id);
        let result = RegistrationResult {
            registration_id: registration.manifest.registration_id.clone(),
            revision,
        };
        let previous = provider.current.replace(registration.clone());
        self.revision = revision;
        self.publication_changed(previous.as_deref(), &registration);
        Ok(result)
    }

    pub fn unregister(
        &mut self,
        connection_id: Uuid,
        registration_id: &str,
    ) -> Result<RegistrationResult, Error> {
        let connection = self
            .connections
            .get(&connection_id)
            .ok_or(Error::Unavailable)?;
        let provider = self
            .providers
            .get_mut(&connection.provider_id)
            .expect("attached provider");
        if provider.active != Some(connection_id)
            || provider
                .current
                .as_ref()
                .is_none_or(|r| r.manifest.registration_id != registration_id)
        {
            return Err(Error::Invalid("registration is not current"));
        }
        let revision = self
            .revision
            .checked_add(1)
            .filter(|r| *r <= 9_007_199_254_740_991)
            .ok_or(Error::Invalid("revision exhausted"))?;
        let previous = provider
            .current
            .take()
            .expect("validated current registration");
        self.revision = revision;
        self.publication_removed(&previous);
        Ok(RegistrationResult {
            registration_id: registration_id.into(),
            revision,
        })
    }

    /// Disconnection marks session bindings Lost, unlike explicit unregister's
    /// removal. Inactive socket teardown cannot clear its replacement.
    pub fn detach(&mut self, id: Uuid) {
        let Some(connection) = self.connections.remove(&id) else {
            return;
        };
        connection.endpoint.close();
        if let Some(provider) = self.providers.get_mut(&connection.provider_id)
            && provider.active == Some(id)
        {
            provider.active = None;
            if provider.current.take().is_some() {
                self.revision += 1;
            }
            self.provider_lost(&connection.provider_id);
        }
        self.prune();
    }

    pub fn begin_drain(&mut self) {
        self.draining = true;
        for connection in self.connections.values() {
            connection.endpoint.close();
        }
        self.connections.clear();
        self.providers.clear();
        self.sessions.clear();
    }

    fn prune(&mut self) {
        self.providers.retain(|id, provider| {
            provider.current.is_some()
                || provider.active.is_some()
                || Arc::strong_count(&provider.identity) > 1
                || self
                    .connections
                    .values()
                    .any(|connection| &connection.provider_id == id)
                || !provider
                    .residency
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty()
        });
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.begin_drain();
    }
}
