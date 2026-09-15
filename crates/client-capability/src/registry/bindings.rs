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

mod selection;
pub use selection::PreparedBindings;
mod snapshot;
pub use snapshot::{Snapshot, SnapshotOffer};

use super::{Registration, Registry};
use crate::{ContractId, Identity};
use maka_runtime::capability::Affinity;
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingMode {
    Strict,
    Degrade,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum BindingError {
    #[error("Session-bound Client Capability provider is unavailable")]
    Lost,
    #[error("Client Capability provider selection is ambiguous")]
    Ambiguous,
    #[error("Client Capability contracts expose conflicting model tool names")]
    Conflict,
    #[error("Client Capability registry is draining")]
    Draining,
    #[error("Required Client Capability tools need one available Session-bound provider")]
    RequiredProvider,
}

/// Retains authentication authority across a lost connection, not publication
/// residency. A reconnect cannot change this provider's credential binding.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderRef {
    id: String,
    identity: Arc<Identity>,
}
impl ProviderRef {
    fn of(registration: &Registration) -> Self {
        Self {
            id: registration.provider_id().into(),
            identity: registration.identity().clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Binding {
    Bound(ProviderRef),
    Lost(ProviderRef),
}
impl Binding {
    fn provider(&self) -> &ProviderRef {
        match self {
            Self::Bound(provider) | Self::Lost(provider) => provider,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct SessionBindings {
    initiating: Option<ProviderRef>,
    service: Option<ProviderRef>,
    session: BTreeMap<ContractId, Binding>,
    turn: BTreeMap<ContractId, ProviderRef>,
}

type Candidates = BTreeMap<ContractId, Vec<Arc<Registration>>>;

impl Registry {
    fn eligible(&self) -> Candidates {
        let mut eligible: Candidates = BTreeMap::new();
        for registration in self.published().filter(|r| r.available()) {
            for (contract, _) in registration.offers() {
                eligible
                    .entry(contract.clone())
                    .or_default()
                    .push(registration.clone());
            }
        }
        for candidates in eligible.values_mut() {
            candidates.sort_by(|a, b| a.provider_id().cmp(b.provider_id()));
        }
        eligible
    }

    pub fn release_session(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
        self.prune();
    }

    pub(super) fn publication_changed(
        &mut self,
        previous: Option<&Registration>,
        current: &Registration,
    ) {
        for state in self.sessions.values_mut() {
            let removed = |contract: &ContractId| {
                previous.is_some_and(|old| old.offer(contract).is_some())
                    && current.offer(contract).is_none()
            };
            state.session.retain(|contract, binding| {
                !matches!(binding, Binding::Bound(provider) if provider.id == current.provider_id() && removed(contract))
            });
            state.turn.retain(|contract, provider| {
                provider.id != current.provider_id() || !removed(contract)
            });
            for (contract, binding) in &mut state.session {
                if let Binding::Lost(provider) = binding
                    && provider.id == current.provider_id()
                    && current.offer(contract).is_some()
                {
                    *binding = Binding::Bound(provider.clone());
                }
            }
        }
        self.prune_sessions();
    }

    pub(super) fn publication_removed(&mut self, previous: &Registration) {
        for state in self.sessions.values_mut() {
            state.session.retain(|contract, binding| {
                !matches!(binding, Binding::Bound(provider)
                    if provider.id == previous.provider_id() && previous.offer(contract).is_some())
            });
            state.turn.retain(|contract, provider| {
                provider.id != previous.provider_id() || previous.offer(contract).is_none()
            });
        }
        self.prune_sessions();
    }

    pub(super) fn provider_lost(&mut self, provider_id: &str) {
        for state in self.sessions.values_mut() {
            for binding in state.session.values_mut() {
                if let Binding::Bound(provider) = binding
                    && provider.id == provider_id
                {
                    *binding = Binding::Lost(provider.clone());
                }
            }
            state.turn.retain(|_, provider| provider.id != provider_id);
        }
        self.prune_sessions();
    }

    fn prune_sessions(&mut self) {
        let has_calls = self
            .published()
            .any(|r| r.available() && r.offers().any(|(_, o)| o.affinity == Affinity::Call));
        self.sessions.retain(|_, state| {
            !state.session.is_empty()
                || !state.turn.is_empty()
                || state.service.is_some()
                || has_calls
        });
    }
}

fn choose(
    candidates: &[Arc<Registration>],
    selector: Option<&ProviderRef>,
) -> Result<Option<Arc<Registration>>, BindingError> {
    if let Some(selector) = selector {
        return Ok(candidates
            .iter()
            .find(|r| r.provider_id() == selector.id)
            .cloned());
    }
    match candidates {
        [] => Ok(None),
        [one] => Ok(Some(one.clone())),
        _ => Err(BindingError::Ambiguous),
    }
}

fn claim_names(
    registration: &Registration,
    contract: &ContractId,
    names: &mut HashSet<String>,
) -> bool {
    let offer = registration.offer(contract).expect("eligible offer");
    let proposed: Vec<_> = offer
        .tools
        .iter()
        .map(|t| crate::proxy_tool_name(&t.server_id, &t.name))
        .collect();
    if proposed.iter().any(|name| names.contains(name)) {
        return false;
    }
    names.extend(proposed);
    true
}
