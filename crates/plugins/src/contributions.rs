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

pub use crate::Registration;
use crate::{
    Error,
    composition::Scope,
    fiber::{CallGuard, Context, Effect, Fiber},
};
use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

type Key = (TypeId, Scope, String);

#[derive(Clone)]
struct Record {
    retired: CancellationToken,
    batch: uuid::Uuid,
    owner: Context,
    value: Arc<dyn Any + Send + Sync>,
}

#[derive(Default)]
struct State {
    records: HashMap<Key, Record>,
    reserved: HashMap<(TypeId, String), Option<String>>,
    host_only: HashSet<TypeId>,
    revision: u64,
}

struct Inner {
    id: uuid::Uuid,
    state: Mutex<State>,
    changed: watch::Sender<u64>,
}

/// One lock publishes a candidate's typed descriptors together. Snapshots pin
/// descriptor/handler pairs, while admission remains bound to the original Fiber.
#[derive(Clone)]
pub struct Catalog(Arc<Inner>);

impl Default for Catalog {
    fn default() -> Self {
        Self(Arc::new(Inner {
            id: uuid::Uuid::new_v4(),
            state: Mutex::new(State::default()),
            changed: watch::channel(0).0,
        }))
    }
}

#[derive(Default)]
pub struct Staged {
    entries: Vec<(TypeId, String, Arc<dyn Any + Send + Sync>)>,
}

impl Staged {
    pub fn insert<T: Send + Sync + 'static>(
        &mut self,
        name: impl Into<String>,
        value: T,
    ) -> Result<(), Error> {
        let name = name.into();
        crate::name(&name)?;
        let kind = TypeId::of::<T>();
        if self
            .entries
            .iter()
            .any(|(ty, key, _)| *ty == kind && *key == name)
        {
            return Err(Error::ContributionConflict(name));
        }
        self.entries.push((kind, name, Arc::new(value)));
        Ok(())
    }
}

impl Catalog {
    pub fn owns(&self, captured: &Captured) -> bool {
        captured.catalog_id == self.0.id
    }
    pub fn host_only<T: Send + Sync + 'static>(&self) -> Result<(), Error> {
        let kind = TypeId::of::<T>();
        let mut state = self.0.state.lock().unwrap();
        if state
            .records
            .keys()
            .any(|(ty, scope, _)| *ty == kind && *scope == Scope::DesktopUi)
        {
            return Err(Error::Invalid(
                "Host-only capability already registered in desktop-ui".into(),
            ));
        }
        state.host_only.insert(kind);
        Ok(())
    }

    /// Host core names are reserved before activating any plugin.
    pub fn reserve<T: Send + Sync + 'static>(&self, name: &str) -> Result<(), Error> {
        self.reserve_name::<T>(name, None)
    }

    /// Transfer a reserved contribution to one designated package, not to every
    /// built-in. Publication and revocation still use the normal Fiber lifecycle.
    pub fn reserve_for<T: Send + Sync + 'static>(
        &self,
        name: &str,
        package: &str,
    ) -> Result<(), Error> {
        crate::name(package)?;
        self.reserve_name::<T>(name, Some(package))
    }

    fn reserve_name<T: Send + Sync + 'static>(
        &self,
        name: &str,
        package: Option<&str>,
    ) -> Result<(), Error> {
        crate::name(name)?;
        let kind = TypeId::of::<T>();
        let mut state = self.0.state.lock().unwrap();
        let key = (kind, name.to_owned());
        if let Some(existing) = state.reserved.get(&key) {
            return if existing.as_deref() == package {
                Ok(())
            } else {
                Err(Error::ContributionConflict(name.into()))
            };
        }
        if state
            .records
            .keys()
            .any(|(ty, _, key)| *ty == kind && key == name)
        {
            return Err(Error::ContributionConflict(name.into()));
        }
        state.reserved.insert(key, package.map(str::to_owned));
        Ok(())
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.changed.subscribe()
    }

    pub fn publish(&self, fiber: &Fiber, staged: Staged) -> Result<(), Error> {
        self.commit(&fiber.context(), staged)
    }

    /// Adopt before publishing: a retiring parent can never leave a visible,
    /// unowned child. The returned Context remains a non-owning capability view.
    pub fn publish_child(
        &self,
        parent: &Context,
        child: Fiber,
        staged: Staged,
    ) -> Result<Context, Error> {
        let context = child.context();
        if let Err(child) = parent.own_child(child) {
            drop(child);
            return Err(Error::Retired);
        }
        if let Err(error) = self.commit(&context, staged) {
            context.retire();
            return Err(error);
        }
        Ok(context)
    }

    pub(crate) fn commit(&self, owner: &Context, staged: Staged) -> Result<(), Error> {
        self.commit_registration(owner, staged)
            .map(Registration::retain)
    }

    pub fn register(&self, owner: &Context, staged: Staged) -> Result<Registration, Error> {
        let _admission = owner.admit()?;
        self.commit_registration(owner, staged)
    }

    pub fn withdraw<T: Send + Sync + 'static>(
        &self,
        owner: &Context,
        name: &str,
    ) -> Result<(), Error> {
        let _admission = owner.admit()?;
        let identity = owner.identity()?;
        let key = (TypeId::of::<T>(), identity.scope, name.to_owned());
        let mut state = self.0.state.lock().unwrap();
        if let Some(record) = state.records.get(&key) {
            if record.owner.identity()?.activation != identity.activation {
                return Err(Error::ContributionConflict(name.into()));
            }
            record.retired.cancel();
            state.records.remove(&key);
            changed(&self.0, &mut state);
        }
        Ok(())
    }

    fn commit_registration(&self, owner: &Context, staged: Staged) -> Result<Registration, Error> {
        let identity = owner.identity()?;
        let scope = identity.scope;
        let batch = uuid::Uuid::new_v4();
        let registry = Arc::downgrade(&self.0);
        let revoke = Effect::new(
            "contribution publication",
            move || {
                if let Some(registry) = registry.upgrade() {
                    let mut state = registry.state.lock().unwrap();
                    let before = state.records.len();
                    state.records.retain(|_, record| {
                        if record.batch == batch {
                            record.retired.cancel();
                            false
                        } else {
                            true
                        }
                    });
                    if before != state.records.len() {
                        changed(&registry, &mut state);
                    }
                }
            },
            || async { Ok(()) },
        );
        // Never drop a revocation Effect while holding the catalog lock.
        let registration = Registration::new(owner, revoke)?;
        let mut state = self.0.state.lock().unwrap();
        for (kind, name, _) in &staged.entries {
            if scope == Scope::DesktopUi && state.host_only.contains(kind) {
                return Err(Error::Invalid(
                    "desktop-ui cannot publish Host capabilities".into(),
                ));
            }
            if state
                .reserved
                .get(&(*kind, name.clone()))
                .is_some_and(|package| package.as_deref() != Some(identity.package_id.as_str()))
                || state
                    .records
                    .contains_key(&(*kind, scope.clone(), name.clone()))
            {
                return Err(Error::ContributionConflict(name.clone()));
            }
        }
        for (kind, name, value) in staged.entries {
            state.records.insert(
                (kind, scope.clone(), name),
                Record {
                    retired: CancellationToken::new(),
                    batch,
                    owner: owner.clone(),
                    value,
                },
            );
        }
        if let Err(error) = owner.publish() {
            state.records.retain(|_, record| record.batch != batch);
            return Err(error);
        }
        changed(&self.0, &mut state);
        Ok(registration)
    }

    pub fn snapshot<T: Send + Sync + 'static>(&self, scope: &Scope) -> Snapshot<T> {
        self.capture_kind(scope, Some(TypeId::of::<T>())).typed()
    }

    /// Host lifecycle consumers visit every effective owner, without Session
    /// shadowing. Callbacks run after the catalog lock has been released.
    pub fn all<T: Send + Sync + 'static>(&self) -> Vec<Contribution<T>> {
        self.0
            .state
            .lock()
            .unwrap()
            .records
            .iter()
            .filter(|((kind, _, _), record)| {
                *kind == TypeId::of::<T>() && record.owner.is_effective()
            })
            .map(|(_, record)| Contribution {
                retired: record.retired.clone(),
                owner: record.owner.clone(),
                value: record
                    .value
                    .clone()
                    .downcast::<T>()
                    .expect("catalog type identity"),
            })
            .collect()
    }

    /// Tools, prompts and other request capabilities share this single capture.
    pub fn capture(&self, scope: &Scope) -> Captured {
        self.capture_kind(scope, None)
    }

    fn capture_kind(&self, scope: &Scope, selected: Option<TypeId>) -> Captured {
        let state = self.0.state.lock().unwrap();
        let mut records = HashMap::new();
        let roots = if matches!(scope, Scope::Session(_)) {
            vec![&Scope::Profile, scope]
        } else {
            vec![scope]
        };
        for root in roots {
            for ((kind, entry_scope, name), record) in &state.records {
                if selected.is_some_and(|selected| selected != *kind)
                    || entry_scope != root
                    || !record.owner.is_effective()
                {
                    continue;
                }
                records.insert((*kind, name.clone()), record.clone());
            }
        }
        Captured {
            catalog_id: self.0.id,
            scope: scope.clone(),
            revision: state.revision,
            records,
        }
    }
}

pub struct Captured {
    catalog_id: uuid::Uuid,
    scope: Scope,
    pub revision: u64,
    records: HashMap<(TypeId, String), Record>,
}

impl Captured {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn typed<T: Send + Sync + 'static>(&self) -> Snapshot<T> {
        let entries = self
            .records
            .iter()
            .filter(|((kind, _), _)| *kind == TypeId::of::<T>())
            .map(|((_, name), record)| {
                (
                    name.clone(),
                    Contribution {
                        retired: record.retired.clone(),
                        value: record
                            .value
                            .clone()
                            .downcast()
                            .expect("captured catalog type matches value"),
                        owner: record.owner.clone(),
                    },
                )
            })
            .collect();
        Snapshot {
            revision: self.revision,
            entries,
        }
    }
}

pub struct Snapshot<T> {
    pub revision: u64,
    pub entries: BTreeMap<String, Contribution<T>>,
}

pub struct Contribution<T> {
    retired: CancellationToken,
    pub value: Arc<T>,
    pub owner: Context,
}

impl<T> Clone for Contribution<T> {
    fn clone(&self) -> Self {
        Self {
            retired: self.retired.clone(),
            value: self.value.clone(),
            owner: self.owner.clone(),
        }
    }
}

impl<T> Contribution<T> {
    pub fn admit(&self) -> Result<CallGuard, Error> {
        let call = self.owner.admit()?;
        if self.retired.is_cancelled() {
            return Err(Error::Retired);
        }
        Ok(call)
    }
    pub fn is_effective(&self) -> bool {
        !self.retired.is_cancelled() && self.owner.is_effective()
    }
    /// Live executors depend on both registration and instance liveness.
    pub async fn retired(&self) {
        let Ok(stopping) = self.owner.stopping() else {
            return;
        };
        tokio::select! {
            _ = stopping.cancelled() => {},
            _ = self.retired.cancelled() => {},
        }
    }
}

fn changed(registry: &Inner, state: &mut State) {
    state.revision = state
        .revision
        .checked_add(1)
        .expect("catalog revision exhausted");
    registry.changed.send_replace(state.revision);
}
