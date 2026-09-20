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

use super::{Dependencies, Kernel, Live, Loading, Plan, PluginContext};
use crate::{
    Error,
    contributions::Staged,
    fiber::{Fiber, Phase},
    services::ServiceView,
};
use std::collections::BTreeSet;
use tokio::{sync::oneshot::error::TryRecvError, time::Instant};

impl Kernel {
    pub(super) fn retire_changed(&mut self) {
        let mut changed = BTreeSet::new();
        for (id, live) in &self.live {
            if self.desired.get(id) != Some(&live.plan) {
                changed.insert(id.clone());
            }
        }
        loop {
            let before = changed.len();
            for (id, live) in &self.live {
                if live
                    .plan
                    .parent
                    .as_ref()
                    .is_some_and(|parent| changed.contains(parent))
                {
                    changed.insert(id.clone());
                }
            }
            if before == changed.len() {
                break;
            }
        }
        for id in changed {
            self.live[&id].context.retire();
        }
        self.services.readiness_changed();
    }

    pub(super) fn retire_dependencies(&self) -> Result<(), Error> {
        // Closing a provider can invalidate another provider, hence iterate to a
        // fixed point before admitting any replacement initialization.
        loop {
            let mut retired = false;
            for live in self.live.values() {
                if !matches!(live.context.phase(), Phase::Active | Phase::Loading) {
                    continue;
                }
                if self.dependencies(&live.plan, &live.services)?.as_ref()
                    != Some(&live.dependencies)
                {
                    live.context.retire();
                    retired = true;
                }
            }
            if !retired {
                return Ok(());
            }
            self.services.readiness_changed();
        }
    }

    pub(super) fn poll_loading(&mut self) {
        for live in self.live.values_mut() {
            let Some(loading) = &mut live.loading else {
                continue;
            };
            if live.context.phase() != Phase::Loading {
                live.loading = None;
                continue;
            }
            let outcome = match loading.result.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Closed) => Some(Err(
                    "plugin initialization task ended without a result".into(),
                )),
                Err(TryRecvError::Empty) if Instant::now() >= loading.deadline => {
                    Some(Err("plugin initialization timed out".into()))
                }
                Err(TryRecvError::Empty) => None,
            };
            let Some(outcome) = outcome else {
                continue;
            };
            live.loading = None;
            let result = outcome.and_then(|staged| {
                live.context
                    .transition(Phase::Loading, Phase::Active)
                    .map_err(|error| error.to_string())?;
                self.catalog
                    .commit(&live.context, staged)
                    .map_err(|error| error.to_string())
            });
            if let Err(error) = result {
                live.error = Some(error);
                live.attempts = live.attempts.saturating_add(1);
                let millis = (250_u64 << live.attempts.saturating_sub(1).min(7)).min(30_000);
                live.retry_at = Instant::now() + std::time::Duration::from_millis(millis);
                live.context.retire();
            } else {
                live.error = None;
                live.attempts = 0;
            }
            self.services.readiness_changed();
        }
    }

    pub(super) fn discard_cleaned(&mut self) {
        self.live.retain(|id, live| {
            live.context.phase() != Phase::Disposed
                || self.desired.get(id).is_some_and(|plan| !plan.disabled)
        });
        self.roots.retain(|root| root.phase() != Phase::Disposed);
    }

    pub(super) fn start_ready(&mut self) -> Result<(), Error> {
        for id in self.order.clone() {
            let plan = self.desired[&id].clone();
            if plan.disabled {
                continue;
            }
            if self.live.get(&id).is_some_and(|live| {
                live.context.phase() != Phase::Disposed
                    || (live.plan == plan && Instant::now() < live.retry_at)
            }) {
                continue;
            }
            let parent = plan.parent.as_ref().and_then(|id| self.live.get(id));
            if plan.parent.is_some()
                && parent.is_none_or(|parent| {
                    matches!(
                        parent.context.phase(),
                        Phase::Unloading | Phase::Disposed | Phase::Failed
                    )
                })
            {
                continue;
            }
            let services = parent
                .map_or_else(|| self.services.view(), |parent| parent.services.clone())
                .derive(&plan.entry)?;
            let fiber = Fiber::observed(
                plan.entry.package_id.as_deref().unwrap_or("maka.structure"),
                &id,
                plan.scope.clone(),
                self.changed.clone(),
            )?;
            let context = fiber.context();
            if let Some(parent) = parent {
                parent
                    .context
                    .own_child(fiber)
                    .map_err(|_| Error::Retired)?;
            } else {
                self.roots.push(fiber);
            }
            let previous = self.live.remove(&id).filter(|live| live.plan == plan);
            self.live.insert(
                id,
                Live {
                    plan,
                    context,
                    services,
                    dependencies: Dependencies::default(),
                    loading: None,
                    attempts: previous.as_ref().map_or(0, |live| live.attempts),
                    error: previous.and_then(|live| live.error),
                    retry_at: Instant::now(),
                },
            );
        }
        for id in self.order.clone() {
            let Some(live) = self.live.get(&id).filter(|live| {
                live.context.phase() == Phase::Pending && live.plan.problem.is_none()
            }) else {
                continue;
            };
            let Some(dependencies) = self.dependencies(&live.plan, &live.services)? else {
                continue;
            };
            let live = self.live.get_mut(&id).unwrap();
            live.dependencies = dependencies;
            live.context.transition(Phase::Pending, Phase::Loading)?;
            let Some(package) = &live.plan.entry.package_id else {
                live.context.transition(Phase::Loading, Phase::Active)?;
                self.catalog.commit(&live.context, Staged::default())?;
                continue;
            };
            let plugin = self.definitions[package].plugin.clone();
            let mut context = PluginContext {
                lifecycle: live.context.clone(),
                services: live.services.bind(live.context.clone()),
                contributions: self.catalog.publisher(live.context.clone()),
                data: self
                    .data
                    .as_ref()
                    .map(|data| data.bind(live.context.clone()))
                    .transpose()?,
                host: None,
            };
            let host = self.host.clone();
            let config = live.plan.entry.config.clone();
            let result = live
                .context
                .spawn_owned("plugin initialization", async move {
                    if let Some(host) = host {
                        context.host = host.bind(context.lifecycle.clone()).await?;
                    }
                    plugin.activate(context, config).await
                })?;
            live.loading = Some(Loading {
                result,
                deadline: Instant::now() + self.activation_timeout,
            });
        }
        Ok(())
    }

    fn dependencies(&self, plan: &Plan, view: &ServiceView) -> Result<Option<Dependencies>, Error> {
        let definition = plan
            .entry
            .package_id
            .as_ref()
            .map(|id| &self.definitions[id]);
        let names = plan.entry.inject.names().chain(
            definition
                .into_iter()
                .flat_map(|definition| definition.inject.iter().map(String::as_str)),
        );
        let Some(services) = view.dependencies(names)? else {
            return Ok(None);
        };
        let mut dependencies = Dependencies {
            services,
            ..Dependencies::default()
        };
        if let Some(definition) = definition {
            for package in &definition.dependencies {
                let identities: Vec<_> = self
                    .live
                    .values()
                    .filter(|other| {
                        other.plan.scope == plan.scope
                            && other.plan.entry.package_id.as_ref() == Some(package)
                            && other.context.is_ready()
                    })
                    .filter_map(|other| {
                        other
                            .context
                            .identity()
                            .ok()
                            .map(|identity| identity.activation)
                    })
                    .collect();
                if identities.is_empty() {
                    return Ok(None);
                }
                dependencies.packages.insert(package.clone(), identities);
            }
        }
        Ok(Some(dependencies))
    }
}
