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
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

mod bindings;
mod mutation;
mod publication;
pub use bindings::{
    BindingError, BindingMode, PreparedBindings, RestoredBindings, Snapshot, SnapshotOffer,
};
pub use publication::Registration;
use publication::Residency;

struct Provider {
    identity: Arc<Identity>,
    residency: Residency,
    current: Option<Arc<Registration>>,
    scoped: HashMap<String, Arc<Registration>>,
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
    retirement_revision: u64,
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
                    scoped: HashMap::new(),
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

    /// Disconnection marks session bindings Lost, unlike explicit unregister's
    /// removal. Inactive socket teardown cannot clear its replacement.
    pub fn detach(&mut self, id: Uuid) {
        let Some(connection) = self.connections.remove(&id) else {
            return;
        };
        connection.endpoint.close();
        let mut lost = Vec::new();
        if let Some(provider) = self.providers.get_mut(&connection.provider_id) {
            if provider.active == Some(id) {
                provider.active = None;
                if let Some(r) = provider.current.take() {
                    lost.push(r);
                }
            }
            provider.scoped.retain(|_, r| {
                if r.connection_id == id {
                    lost.push(r.clone());
                    false
                } else {
                    true
                }
            });
        }
        for r in lost {
            self.revision += 1;
            self.provider_lost(&r);
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
                || !provider.scoped.is_empty()
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
