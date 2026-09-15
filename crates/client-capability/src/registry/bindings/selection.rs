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
    Binding, BindingError, BindingMode, ProviderRef, SessionBindings, choose, claim_names,
};
use crate::{PrincipalKind, Registry};
use maka_runtime::capability::Affinity;
use std::collections::{BTreeSet, HashSet};
use uuid::Uuid;

/// A candidate owns no Session binding. Commit verifies the same selection and
/// publications under the registry lock before making it authoritative.
pub struct PreparedBindings {
    session_id: String,
    initiating: Option<Uuid>,
    mode: BindingMode,
    previous: Option<SessionBindings>,
    selected: SessionBindings,
    snapshot: super::Snapshot,
}

impl Registry {
    pub fn prepare_bindings(
        &self,
        session_id: &str,
        initiating: Option<Uuid>,
        mode: BindingMode,
    ) -> Result<(PreparedBindings, super::Snapshot), BindingError> {
        let previous = self.sessions.get(session_id).cloned();
        let selected = self.select_bindings(previous.as_ref(), initiating, mode)?;
        let snapshot = self.snapshot_bindings(Some(&selected))?;
        Ok((
            PreparedBindings {
                session_id: session_id.into(),
                initiating,
                mode,
                previous,
                selected,
                snapshot: snapshot.clone(),
            },
            snapshot,
        ))
    }

    /// False means stale preparation; no binding was changed.
    pub fn commit_bindings(&mut self, prepared: PreparedBindings) -> Result<bool, BindingError> {
        if self.sessions.get(&prepared.session_id) != prepared.previous.as_ref() {
            return Ok(false);
        }
        let selected = self.select_bindings(
            prepared.previous.as_ref(),
            prepared.initiating,
            prepared.mode,
        )?;
        if selected != prepared.selected
            || !self
                .snapshot_bindings(Some(&selected))?
                .same_bindings(&prepared.snapshot)
        {
            return Ok(false);
        }
        self.sessions.insert(prepared.session_id, selected);
        self.prune_sessions();
        Ok(true)
    }

    /// Required tools must share one Session-affinity owner. Selection and
    /// snapshot validation finish before any binding becomes visible.
    pub fn bind_required_tools(
        &mut self,
        session_id: &str,
        initiating: Uuid,
        required: &[&str],
    ) -> Result<super::Snapshot, BindingError> {
        let next = self.select_bindings(
            self.sessions.get(session_id),
            Some(initiating),
            BindingMode::Strict,
        )?;
        let mut missing: HashSet<_> = required.iter().copied().collect();
        let mut selected = None;
        for (contract, binding) in &next.session {
            let Binding::Bound(provider) = binding else {
                continue;
            };
            let Some(registration) = self
                .providers
                .get(&provider.id)
                .and_then(|provider| provider.current.as_ref())
                .filter(|r| r.available())
            else {
                continue;
            };
            let Some(offer) = registration.offer(contract) else {
                continue;
            };
            for tool in &offer.tools {
                if missing.remove(crate::proxy_tool_name(&tool.server_id, &tool.name).as_str()) {
                    if selected.is_some_and(|selected| selected != provider) {
                        return Err(BindingError::RequiredProvider);
                    }
                    selected = Some(provider);
                }
            }
        }
        if selected.is_none() || !missing.is_empty() {
            return Err(BindingError::RequiredProvider);
        }
        let snapshot = self.snapshot_bindings(Some(&next))?;
        self.sessions.insert(session_id.into(), next);
        self.prune_sessions();
        Ok(snapshot)
    }

    /// Select all bindings atomically. A failed strict selection changes no
    /// Session state; callers serialize this with invocation admission.
    pub fn bind_session(
        &mut self,
        session_id: &str,
        initiating: Option<Uuid>,
        mode: BindingMode,
    ) -> Result<bool, BindingError> {
        let next = self.select_bindings(self.sessions.get(session_id), initiating, mode)?;
        let changed = self.sessions.get(session_id) != Some(&next);
        self.sessions.insert(session_id.into(), next);
        self.prune_sessions();
        Ok(changed)
    }

    /// Computes the same selection as admission without creating or changing a
    /// Session binding. `None` previews a new Session, not a synthetic identity.
    pub fn preview_bindings(
        &self,
        session_id: Option<&str>,
        initiating: Option<Uuid>,
        mode: BindingMode,
    ) -> Result<super::Snapshot, BindingError> {
        let previous = session_id.and_then(|id| self.sessions.get(id));
        let selected = self.select_bindings(previous, initiating, mode)?;
        self.snapshot_bindings(Some(&selected))
    }

    fn select_bindings(
        &self,
        previous: Option<&SessionBindings>,
        initiating: Option<Uuid>,
        mode: BindingMode,
    ) -> Result<SessionBindings, BindingError> {
        if self.draining {
            return Err(BindingError::Draining);
        }
        let initiating = initiating
            .and_then(|id| self.connections.get(&id))
            .and_then(|connection| self.providers.get(&connection.provider_id));
        let direct = initiating
            .and_then(|provider| provider.current.as_ref())
            .filter(|r| r.available());
        let first_associated = {
            let mut associated = self.published().filter(|r| {
                let Some(initiating) = initiating else {
                    return false;
                };
                let identity = &initiating.identity;
                identity.credential_bound_client_instance_id.as_deref()
                    == Some(identity.client_instance_id.as_str())
                    && r.available()
                    && r.identity().trusted()
                    && r.identity().capability_owner.as_ref().is_some_and(|owner| {
                        owner.principal_id == identity.principal_id
                            && owner.client_instance_id == identity.client_instance_id
                    })
            });
            let first_associated = associated.next();
            if direct.is_none() && first_associated.is_some() && associated.next().is_some() {
                return Err(BindingError::Ambiguous);
            }
            first_associated
        };
        let selected = direct.or(first_associated);
        let selector = selected.map(|r| ProviderRef::of(r)).or_else(|| {
            initiating
                .filter(|p| p.identity.principal_kind == PrincipalKind::RemoteOwner)
                .map(|p| ProviderRef {
                    id: p.identity.provider_id(),
                    identity: p.identity.clone(),
                })
        });
        let mut next = SessionBindings {
            initiating: selector,
            service: previous.and_then(|s| s.service.clone()).or_else(|| {
                selected
                    .filter(|r| {
                        r.manifest()
                            .services
                            .as_ref()
                            .is_some_and(|s| !s.is_empty())
                    })
                    .map(|r| ProviderRef::of(r))
            }),
            ..SessionBindings::default()
        };
        let eligible = self.eligible();
        let contracts: BTreeSet<_> = previous
            .into_iter()
            .flat_map(|s| s.session.keys())
            .chain(
                eligible
                    .iter()
                    .filter(|(id, c)| c[0].offer(id).unwrap().affinity == Affinity::Session)
                    .map(|(id, _)| id),
            )
            .cloned()
            .collect();
        let mut names = HashSet::new();
        for contract in contracts {
            let prior = previous.and_then(|s| s.session.get(&contract));
            let candidates = eligible
                .get(&contract)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let candidate = if let Some(prior) = prior {
                match choose(candidates, Some(prior.provider()))? {
                    Some(candidate) => Some(candidate),
                    None if mode == BindingMode::Degrade => {
                        next.session
                            .insert(contract, Binding::Lost(prior.provider().clone()));
                        continue;
                    }
                    None => return Err(BindingError::Lost),
                }
            } else {
                match choose(candidates, next.initiating.as_ref()) {
                    Err(_) if mode == BindingMode::Degrade => continue,
                    result => result?,
                }
            };
            let Some(candidate) = candidate else {
                continue;
            };
            if !claim_names(&candidate, &contract, &mut names) {
                if mode == BindingMode::Strict {
                    return Err(BindingError::Conflict);
                }
                if prior.is_some() {
                    next.session
                        .insert(contract, Binding::Lost(ProviderRef::of(&candidate)));
                }
                continue;
            }
            next.session
                .insert(contract, Binding::Bound(ProviderRef::of(&candidate)));
        }
        for (contract, candidates) in &eligible {
            if candidates[0].offer(contract).unwrap().affinity != Affinity::Turn {
                continue;
            }
            let Ok(Some(candidate)) = choose(candidates, next.initiating.as_ref()) else {
                continue;
            };
            if claim_names(&candidate, contract, &mut names) {
                next.turn
                    .insert(contract.clone(), ProviderRef::of(&candidate));
            }
        }
        Ok(next)
    }
}
