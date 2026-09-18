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

mod plan;
mod reconcile;

use crate::{
    Error,
    composition::{Composition, Entry, Scope},
    contributions::{Catalog, Staged},
    fiber::{Context, Fiber, Phase},
    services::{ServiceView, Services},
};
use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{sync::oneshot, time::Instant};

/// Statically linked Rust and loaded JS packages implement the same entrypoint.
pub trait Plugin: Send + Sync {
    fn supports_scope(&self, scope: &Scope) -> bool {
        !matches!(scope, Scope::DesktopUi)
    }

    fn validate(&self, _scope: &Scope, _config: &Value) -> Result<(), Error> {
        Ok(())
    }
    fn activate(
        &self,
        context: PluginContext,
        config: Value,
    ) -> BoxFuture<'static, Result<Staged, String>>;
}

#[derive(Clone)]
pub struct PluginContext {
    pub lifecycle: Context,
    pub services: ServiceView,
}

pub struct Definition {
    pub id: String,
    pub revision: String,
    pub dependencies: Vec<String>,
    pub inject: Vec<String>,
    pub plugin: Arc<dyn Plugin>,
}

pub type Definitions = BTreeMap<String, Arc<Definition>>;

pub struct Prepared {
    definitions: Definitions,
    desired: BTreeMap<String, Plan>,
    order: Vec<String>,
}

impl Prepared {
    pub fn new(composition: &Composition, definitions: Definitions) -> Result<Self, Error> {
        let (desired, order) = plan::prepare(composition, &definitions, plan::Validation::Strict)?;
        Ok(Self {
            definitions,
            desired,
            order,
        })
    }

    pub fn recover(composition: &Composition, definitions: Definitions) -> Result<Self, Error> {
        let (desired, order) =
            plan::prepare(composition, &definitions, plan::Validation::Recovery)?;
        Ok(Self {
            definitions,
            desired,
            order,
        })
    }
}

#[derive(Clone, PartialEq)]
struct Plan {
    scope: Scope,
    parent: Option<String>,
    entry: Entry,
    disabled: bool,
    revision: Option<String>,
    problem: Option<String>,
}

struct Loading {
    result: oneshot::Receiver<Result<Staged, String>>,
    deadline: Instant,
}

struct Live {
    plan: Plan,
    context: Context,
    services: ServiceView,
    dependencies: Dependencies,
    loading: Option<Loading>,
    error: Option<String>,
    attempts: u32,
    retry_at: Instant,
}

#[derive(Default, PartialEq)]
struct Dependencies {
    services: BTreeMap<String, uuid::Uuid>,
    packages: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryStatus {
    pub waiting_for: Vec<String>,
    pub effects: Vec<String>,
    pub entry_id: String,
    pub scope: Scope,
    pub disabled: bool,
    pub phase: Phase,
    pub activation: Option<String>,
    pub generation: Option<u64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub converged: bool,
    pub cleanup_complete: bool,
    pub entries: Vec<EntryStatus>,
}

/// A caller-owned reconciler. Each tick is nonblocking; initialization and cleanup
/// stay owned by Fibers. Host serializes intent changes and ticks in its owner task.
pub struct Kernel {
    changed: tokio::sync::watch::Sender<()>,
    services: Services,
    catalog: Catalog,
    definitions: Definitions,
    desired: BTreeMap<String, Plan>,
    order: Vec<String>,
    live: BTreeMap<String, Live>,
    roots: Vec<Fiber>,
    activation_timeout: Duration,
}

impl Kernel {
    pub fn new(services: Services, catalog: Catalog) -> Self {
        Self {
            changed: tokio::sync::watch::channel(()).0,
            services,
            catalog,
            definitions: BTreeMap::new(),
            desired: BTreeMap::new(),
            order: Vec::new(),
            live: BTreeMap::new(),
            roots: Vec::new(),
            activation_timeout: Duration::from_secs(10),
        }
    }

    /// Static validation completes before any current instance is retired.
    pub fn configure(
        &mut self,
        composition: &Composition,
        definitions: Definitions,
    ) -> Result<(), Error> {
        self.install(Prepared::new(composition, definitions)?);
        Ok(())
    }

    pub fn install(&mut self, prepared: Prepared) {
        self.desired = prepared.desired;
        self.order = prepared.order;
        self.definitions = prepared.definitions;
        self.retire_changed();
    }

    pub fn reload(&mut self, package: &str) {
        for live in self.live.values() {
            if live.plan.entry.package_id.as_deref() == Some(package) {
                live.context.retire();
            }
        }
        self.services.readiness_changed();
    }

    pub fn definitions(&self) -> &Definitions {
        &self.definitions
    }

    /// An unchanged recovery failure must not prevent repairing another Entry.
    /// New invalid configuration still fails before durable intent is changed.
    pub fn validate_change(&self, prepared: &Prepared) -> Result<(), Error> {
        for (id, next) in &prepared.desired {
            if !next.disabled && next.problem.is_some() && self.desired.get(id) != Some(next) {
                return Err(Error::Invalid(next.problem.clone().unwrap()));
            }
        }
        Ok(())
    }

    pub fn next_tick(&self) -> Option<Instant> {
        self.live
            .values()
            .filter_map(|live| match live.context.phase() {
                Phase::Loading => live.loading.as_ref().map(|loading| loading.deadline),
                Phase::Disposed
                    if self
                        .desired
                        .get(&live.plan.entry.id)
                        .is_some_and(|plan| !plan.disabled) =>
                {
                    Some(live.retry_at)
                }
                _ => None,
            })
            .min()
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub fn tick(&mut self) -> Result<Status, Error> {
        self.retire_dependencies()?;
        self.poll_loading();
        self.discard_cleaned();
        self.start_ready()?;
        Ok(self.status())
    }

    pub fn status(&self) -> Status {
        let mut entries = Vec::new();
        let mut converged = true;
        let mut cleanup_complete = true;
        for id in &self.order {
            let plan = &self.desired[id];
            let live = self.live.get(id);
            let phase = if plan.problem.is_some() && !plan.disabled {
                Phase::Failed
            } else {
                live.map_or(Phase::Pending, |live| live.context.phase())
            };
            let effective =
                live.is_some_and(|live| live.plan == *plan && live.context.is_effective());
            if !plan.disabled && !effective {
                converged = false;
            }
            if plan.disabled && live.is_some() {
                converged = false;
            }
            entries.push(EntryStatus {
                waiting_for: self.waiting_for(plan, live),
                effects: live.map_or_else(Vec::new, |live| live.context.effect_labels()),
                generation: live
                    .and_then(|live| live.context.identity().ok())
                    .map(|id| id.generation),
                entry_id: id.clone(),
                scope: plan.scope.clone(),
                disabled: plan.disabled,
                phase,
                activation: live
                    .and_then(|live| live.context.identity().ok())
                    .map(|identity| identity.activation),
                error: plan.problem.clone().or_else(|| {
                    live.and_then(|live| {
                        live.context
                            .cleanup_failure()
                            .or_else(|| live.error.clone())
                    })
                }),
            });
        }
        for (id, live) in &self.live {
            if matches!(live.context.phase(), Phase::Unloading | Phase::Failed) {
                cleanup_complete = false;
            }
            if !self.desired.contains_key(id) {
                converged = false;
                entries.push(EntryStatus {
                    waiting_for: Vec::new(),
                    effects: live.context.effect_labels(),
                    generation: live.context.identity().ok().map(|id| id.generation),
                    entry_id: id.clone(),
                    scope: live.plan.scope.clone(),
                    disabled: true,
                    phase: live.context.phase(),
                    activation: live
                        .context
                        .identity()
                        .ok()
                        .map(|identity| identity.activation),
                    error: live
                        .context
                        .cleanup_failure()
                        .or_else(|| live.error.clone()),
                });
            }
        }
        Status {
            converged,
            cleanup_complete,
            entries,
        }
    }

    fn waiting_for(&self, plan: &Plan, live: Option<&Live>) -> Vec<String> {
        if plan.disabled
            || plan.problem.is_some()
            || live.is_some_and(|live| live.context.phase() != Phase::Pending)
        {
            return Vec::new();
        }
        let definition = plan
            .entry
            .package_id
            .as_ref()
            .and_then(|id| self.definitions.get(id));
        let names = plan.entry.inject.names().chain(
            definition
                .into_iter()
                .flat_map(|definition| definition.inject.iter().map(String::as_str)),
        );
        names
            .filter(|name| {
                live.is_none_or(|live| live.services.dependencies([*name]).ok().flatten().is_none())
            })
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn retire(&mut self) {
        self.desired.clear();
        self.order.clear();
        for root in &self.roots {
            root.retire();
        }
    }

    pub async fn shutdown(&mut self, deadline: Instant) -> Result<(), Error> {
        self.retire();
        let mut failures = Vec::new();
        for root in &self.roots {
            match root.shutdown(deadline).await {
                Ok(()) => {}
                Err(Error::CleanupPending) => return Err(Error::CleanupPending),
                Err(error) => failures.push(error.to_string()),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::Cleanup(failures))
        }
    }
}
