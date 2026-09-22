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

use super::{Registration, Registry};
use crate::{Error, PrincipalKind};
use maka_runtime::capability::{Affinity, HostPathAccess, Manifest, RegistrationResult};
use std::{collections::HashSet, sync::Arc};
use uuid::Uuid;

impl Registry {
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
        if (connection.superseded && manifest.session_id.is_none())
            || connection.endpoint.closed().is_cancelled()
        {
            return Err(Error::Invalid("connection has been superseded or closed"));
        }
        let provider = self
            .providers
            .get_mut(&connection.provider_id)
            .expect("attached provider");
        if let Some(session_id) = &manifest.session_id {
            if manifest.services.as_ref().is_some_and(|s| !s.is_empty())
                || manifest.offers.iter().any(|o| {
                    o.affinity != Affinity::Session || o.host_path_access != HostPathAccess::None
                })
            {
                return Err(Error::Invalid(
                    "Session publications support only path-independent Session tools",
                ));
            }
            if !provider.scoped.contains_key(session_id) && provider.scoped.len() >= 128 {
                return Err(Error::Invalid("too many Session publications"));
            }
        }
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
        let overlaps = |r: &Registration| {
            r.manifest
                .offers
                .iter()
                .flat_map(|o| &o.tools)
                .any(|t| proxy_names.contains(&crate::proxy_tool_name(&t.server_id, &t.name)))
        };
        if if manifest.session_id.is_some() {
            provider.current.as_ref().is_some_and(|r| overlaps(r))
        } else {
            provider.scoped.values().any(|r| overlaps(r))
        } {
            return Err(Error::Invalid(
                "global and Session publications expose overlapping tools",
            ));
        }
        let mut resident = provider.residency.lock().unwrap_or_else(|e| e.into_inner());
        if resident.contains_key(&manifest.registration_id) {
            return Err(Error::Invalid("registration identity is still resident"));
        }
        let invocations = if manifest.session_id.is_some() {
            connection.endpoint.closed().child_token()
        } else {
            connection.endpoint.invocations().child_token()
        };
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
            invocations,
        });
        resident.insert(
            registration.manifest.registration_id.clone(),
            Arc::downgrade(&registration),
        );
        drop(resident);
        if registration.session_id().is_none()
            && let Some(old) = provider.active.filter(|old| *old != connection_id)
            && let Some(connection) = self.connections.get_mut(&old)
        {
            connection.superseded = true;
            connection.endpoint.supersede();
        }
        let result = RegistrationResult {
            registration_id: registration.manifest.registration_id.clone(),
            revision,
        };
        let previous = match registration.session_id() {
            Some(id) => provider.scoped.insert(id.into(), registration.clone()),
            None => {
                provider.active = Some(connection_id);
                provider.current.replace(registration.clone())
            }
        };
        // Explicit withdrawal can leave pinned generations without a current
        // slot. A new connection still takes over their publication scope.
        let resident: Vec<_> = provider
            .residency
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter_map(std::sync::Weak::upgrade)
            .collect();
        for r in resident {
            if r.session_id() == registration.session_id() && r.connection_id != connection_id {
                r.invocations.cancel();
            }
        }
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
        let previous = provider
            .current
            .iter()
            .chain(provider.scoped.values())
            .find(|r| {
                r.connection_id == connection_id && r.manifest.registration_id == registration_id
            })
            .cloned()
            .ok_or(Error::Invalid("registration is not current"))?;
        let revision = self
            .revision
            .checked_add(1)
            .filter(|r| *r <= 9_007_199_254_740_991)
            .ok_or(Error::Invalid("revision exhausted"))?;
        match previous.session_id() {
            Some(id) => {
                provider.scoped.remove(id);
            }
            None => {
                provider.current.take();
            }
        }
        self.revision = revision;
        self.publication_removed(&previous);
        Ok(RegistrationResult {
            registration_id: registration_id.into(),
            revision,
        })
    }
}
