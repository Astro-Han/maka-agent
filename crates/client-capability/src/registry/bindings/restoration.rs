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

use super::{
    Binding, BindingError, ProviderRef, SessionBindings, Snapshot, SnapshotOffer, snapshot::Source,
};
use crate::{ContractId, Identity, Registry};
use maka_runtime::capability::{Affinity, ClientComposition, ClientOffer, PinnedAffinity};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

/// Restores only the admitted surface. It never discovers additional tools.
pub struct RestoredBindings {
    session_id: String,
    previous: Option<SessionBindings>,
    selected: SessionBindings,
    identities: BTreeMap<String, Arc<Identity>>,
    snapshot: Snapshot,
}

impl Registry {
    pub fn restore_bindings(
        &self,
        session_id: &str,
        composition: &ClientComposition,
    ) -> Result<(RestoredBindings, Snapshot), BindingError> {
        if self.draining {
            return Err(BindingError::Draining);
        }
        let mut identities = BTreeMap::new();
        let mut selected = SessionBindings::default();
        for (contract, identity) in &composition.session_bindings {
            let provider = self.restore_identity(identity, &mut identities)?;
            let available = self.current(&provider.id).is_some_and(|r| {
                r.available()
                    && r.offer(contract)
                        .is_some_and(|o| o.affinity == Affinity::Session)
            });
            selected.session.insert(
                contract.clone(),
                if available {
                    Binding::Bound(provider)
                } else {
                    Binding::Lost(provider)
                },
            );
        }
        let mut offers = Vec::with_capacity(composition.offers.len());
        let mut names = HashSet::new();
        let mut contracts = HashSet::new();
        for entry in &composition.offers {
            let (contract, source) = match entry {
                ClientOffer::Pinned {
                    contract,
                    affinity,
                    identity,
                } => {
                    let provider = self.restore_identity(identity, &mut identities)?;
                    let registration = self
                        .current(&provider.id)
                        .filter(|r| r.available())
                        .ok_or(BindingError::Lost)?;
                    let offer = registration.offer(contract).ok_or(BindingError::Lost)?;
                    match affinity {
                        PinnedAffinity::Session => {
                            if offer.affinity != Affinity::Session
                                || selected.session.get(contract).map(Binding::provider)
                                    != Some(&provider)
                            {
                                return Err(BindingError::InvalidComposition);
                            }
                        }
                        PinnedAffinity::Turn => {
                            if offer.affinity != Affinity::Turn {
                                return Err(BindingError::InvalidComposition);
                            }
                            selected.turn.insert(contract.clone(), provider);
                        }
                    }
                    (contract.clone(), Source::Pinned(registration))
                }
                ClientOffer::Call { offer, selector } => {
                    // Durable decoding does not replace manifest admission validation.
                    let manifest = serde_json::json!({"registrationId":"restore","offers":[offer]});
                    if offer.affinity != Affinity::Call
                        || maka_protocol::capability::decode_replace_input(&manifest).is_err()
                    {
                        return Err(BindingError::InvalidComposition);
                    }
                    let selector = selector
                        .as_ref()
                        .map(|identity| self.restore_identity(identity, &mut identities))
                        .transpose()?;
                    (
                        ContractId::of(offer),
                        Source::Call {
                            offer: offer.clone(),
                            selector,
                        },
                    )
                }
            };
            if !contracts.insert(contract.clone()) {
                return Err(BindingError::InvalidComposition);
            }
            let entry = SnapshotOffer { contract, source };
            for tool in &entry.offer().tools {
                if !names.insert(crate::proxy_tool_name(&tool.server_id, &tool.name)) {
                    return Err(BindingError::Conflict);
                }
            }
            offers.push(entry);
        }
        let snapshot = Snapshot { offers };
        Ok((
            RestoredBindings {
                session_id: session_id.into(),
                previous: self.sessions.get(session_id).cloned(),
                selected,
                identities,
                snapshot: snapshot.clone(),
            },
            snapshot,
        ))
    }

    /// Caller serializes this with canonical invocation admission. A stale
    /// candidate has no effect, including no offline authentication records.
    pub fn commit_restored_bindings(
        &mut self,
        restored: RestoredBindings,
    ) -> Result<bool, BindingError> {
        if self.draining {
            return Err(BindingError::Draining);
        }
        if self.sessions.get(&restored.session_id) != restored.previous.as_ref() {
            return Ok(false);
        }
        for (id, identity) in &restored.identities {
            if let Some(provider) = self.providers.get(id) {
                if provider.identity != *identity {
                    return Err(BindingError::InvalidComposition);
                }
                // A concurrent attach created another identity owner. Reprepare
                // so all references share the registry's authentication lifetime.
                if !Arc::ptr_eq(&provider.identity, identity) {
                    return Ok(false);
                }
            }
        }
        for entry in &restored.snapshot.offers {
            if let Source::Pinned(pinned) = &entry.source
                && (!pinned.available()
                    || !self
                        .current(pinned.provider_id())
                        .is_some_and(|current| Arc::ptr_eq(&current, pinned)))
            {
                return Ok(false);
            }
        }
        for (id, identity) in restored.identities {
            self.providers.entry(id).or_insert(super::super::Provider {
                identity,
                residency: Arc::default(),
                current: None,
                active: None,
            });
        }
        self.sessions.insert(restored.session_id, restored.selected);
        Ok(true)
    }

    fn restore_identity(
        &self,
        identity: &Identity,
        identities: &mut BTreeMap<String, Arc<Identity>>,
    ) -> Result<ProviderRef, BindingError> {
        if identity.capability_owner.is_some()
            && identity.principal_kind != crate::PrincipalKind::CapabilityProvider
        {
            return Err(BindingError::InvalidComposition);
        }
        let id = identity.provider_id();
        let existing = identities
            .get(&id)
            .cloned()
            .or_else(|| self.providers.get(&id).map(|p| p.identity.clone()));
        let stored = match existing {
            Some(stored) if *stored != *identity => return Err(BindingError::InvalidComposition),
            Some(stored) => stored,
            None => Arc::new(identity.clone()),
        };
        identities.insert(id.clone(), stored.clone());
        Ok(ProviderRef {
            id,
            identity: stored,
        })
    }
}
