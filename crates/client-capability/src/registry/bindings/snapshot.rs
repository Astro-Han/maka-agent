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

use super::{Binding, BindingError, ProviderRef, choose, claim_names};
use crate::ManagedAdmissionError;
use crate::{ContractId, Registration, Registry};
use maka_runtime::capability::{Affinity, Offer};
use maka_runtime::{capability::AdmissionEvidence, interaction::GrantTarget};
use std::{collections::HashSet, sync::Arc};

#[derive(Clone)]
pub struct Snapshot {
    pub(super) offers: Vec<SnapshotOffer>,
}
impl Snapshot {
    pub(super) fn composition(&self) -> Vec<maka_runtime::capability::ClientOffer> {
        use maka_runtime::capability::{ClientOffer, PinnedAffinity};
        self.offers
            .iter()
            .map(|entry| match &entry.source {
                Source::Pinned(registration) => ClientOffer::Pinned {
                    contract: entry.contract.clone(),
                    affinity: match entry.offer().affinity {
                        Affinity::Session => PinnedAffinity::Session,
                        Affinity::Turn => PinnedAffinity::Turn,
                        Affinity::Call => unreachable!("Call offers are not pinned"),
                    },
                    publication: ProviderRef::of(registration).publication(),
                },
                Source::Call { offer, selector } => ClientOffer::Call {
                    offer: offer.clone(),
                    selector: selector.as_ref().map(|p| (*p.identity).clone()),
                },
            })
            .collect()
    }

    pub fn offers(&self) -> &[SnapshotOffer] {
        &self.offers
    }

    pub(super) fn same_bindings(&self, other: &Self) -> bool {
        self.offers.len() == other.offers.len()
            && self.offers.iter().zip(&other.offers).all(|(a, b)| {
                a.contract == b.contract
                    && match (&a.source, &b.source) {
                        (Source::Pinned(a), Source::Pinned(b)) => Arc::ptr_eq(a, b),
                        (
                            Source::Call {
                                offer: a,
                                selector: sa,
                            },
                            Source::Call {
                                offer: b,
                                selector: sb,
                            },
                        ) => a == b && sa == sb,
                        _ => false,
                    }
            })
    }
}

#[derive(Clone)]
pub struct SnapshotOffer {
    pub(super) contract: ContractId,
    pub(super) source: Source,
}
#[derive(Clone)]
pub(super) enum Source {
    Pinned(Arc<Registration>),
    Call {
        offer: Offer,
        selector: Option<ProviderRef>,
    },
}
impl SnapshotOffer {
    pub fn contract_id(&self) -> &ContractId {
        &self.contract
    }
    pub fn offer(&self) -> &Offer {
        match &self.source {
            Source::Pinned(registration) => registration
                .offer(&self.contract)
                .expect("snapshot contract"),
            Source::Call { offer, .. } => offer,
        }
    }
    pub fn trusted(&self) -> bool {
        matches!(&self.source, Source::Pinned(registration) if registration.identity().trusted())
    }
    /// Derives a managed grant from this frozen binding, without admitting an
    /// invocation or consulting mutable registry state. The caller retains the
    /// exact registration resolved before provider preparation, including for
    /// call affinity. Settings need no grant.
    pub fn managed_target(
        &self,
        registration: &Registration,
        server_id: &str,
        tool_name: &str,
        evidence: &AdmissionEvidence,
    ) -> Result<Option<GrantTarget>, ManagedAdmissionError> {
        if !registration.identity().trusted() {
            return Err(ManagedAdmissionError::UntrustedProvider);
        }
        if let Source::Pinned(pinned) = &self.source
            && !std::ptr::eq(pinned.as_ref(), registration)
        {
            return Err(ManagedAdmissionError::WrongRegistration);
        }
        let offer = registration
            .offer(&self.contract)
            .ok_or(ManagedAdmissionError::WrongRegistration)?;
        let tool = offer
            .tools
            .iter()
            .find(|tool| tool.server_id == server_id && tool.name == tool_name)
            .ok_or(ManagedAdmissionError::UnknownTool)?;
        let Some((capability, scope)) = crate::managed::scope(offer, tool, evidence)? else {
            return Ok(None);
        };
        Ok(Some(GrantTarget {
            provider_id: registration.provider_id().into(),
            contract_id: self.contract.as_str().into(),
            server_id: tool.server_id.clone(),
            tool_name: tool.name.clone(),
            capability,
            scope,
        }))
    }
    /// Call affinity resolves each invocation; it never pins a representative
    /// publication or silently switches a previously admitted invocation.
    pub fn resolve(&self, registry: &Registry) -> Result<Arc<Registration>, BindingError> {
        match &self.source {
            Source::Pinned(registration) => {
                if registration.available() {
                    Ok(registration.clone())
                } else {
                    Err(BindingError::Lost)
                }
            }
            Source::Call { selector, .. } => {
                let eligible = registry.eligible(None);
                choose(
                    eligible
                        .get(&self.contract)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    selector.as_ref(),
                )?
                .ok_or(BindingError::Lost)
            }
        }
    }
}
impl Registry {
    pub fn snapshot(&self, session_id: &str) -> Result<Snapshot, BindingError> {
        self.snapshot_bindings(self.sessions.get(session_id))
    }

    pub(super) fn snapshot_bindings(
        &self,
        state: Option<&super::SessionBindings>,
    ) -> Result<Snapshot, BindingError> {
        if self.draining {
            return Err(BindingError::Draining);
        }
        let mut offers = Vec::new();
        let mut names = HashSet::new();
        if let Some(state) = state {
            for (contract, binding) in &state.session {
                let Binding::Bound(provider) = binding else {
                    continue;
                };
                let registration = self
                    .current_scoped(&provider.id, provider.session_id.as_deref())
                    .filter(|r| r.available() && r.offer(contract).is_some())
                    .ok_or(BindingError::Lost)?;
                if !claim_names(&registration, contract, &mut names) {
                    return Err(BindingError::Conflict);
                }
                offers.push(SnapshotOffer {
                    contract: contract.clone(),
                    source: Source::Pinned(registration),
                });
            }
            for (contract, provider) in &state.turn {
                let Some(registration) = self
                    .current_scoped(&provider.id, provider.session_id.as_deref())
                    .filter(|r| r.available() && r.offer(contract).is_some())
                else {
                    continue;
                };
                if claim_names(&registration, contract, &mut names) {
                    offers.push(SnapshotOffer {
                        contract: contract.clone(),
                        source: Source::Pinned(registration),
                    });
                }
            }
        }
        for (contract, candidates) in self.eligible(None) {
            let selector = state.and_then(|s| s.initiating.as_ref());
            let candidate = if selector.is_some() {
                choose(&candidates, selector)?
            } else {
                candidates.first().cloned()
            };
            let Some(candidate) = candidate else {
                continue;
            };
            let offer = candidate.offer(&contract).expect("eligible offer");
            if offer.affinity != Affinity::Call || !claim_names(&candidate, &contract, &mut names) {
                continue;
            }
            offers.push(SnapshotOffer {
                contract,
                source: Source::Call {
                    offer: offer.clone(),
                    selector: selector.cloned(),
                },
            });
        }
        Ok(Snapshot { offers })
    }
}
