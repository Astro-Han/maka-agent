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
    reserved: HashSet<(TypeId, String)>,
    host_only: HashSet<TypeId>,
    revision: u64,
}

struct Inner {
    id: uuid::Uuid,
    calls: Option<crate::call::Issuer>,
    state: Mutex<State>,
    changed: watch::Sender<u64>,
}

/// One lock publishes a candidate's typed descriptors together. Snapshots pin
/// descriptor/handler pairs, while admission remains bound to the original Fiber.
#[derive(Clone)]
pub struct Catalog(Arc<Inner>);

/// Publication capability bound to one activation. Plugins can change their
/// own contributions, but cannot choose another owner or inspect the catalog.
#[derive(Clone)]
pub struct Publisher {
    catalog: Catalog,
    owner: Context,
}
impl Publisher {
    /// Initialization returns Staged instead; dynamic publication begins only
    /// after this activation has been made effective.
    pub fn publish(&self, staged: Staged) -> Result<Registration, Error> {
        self.catalog.register(&self.owner, staged)
    }

    pub fn withdraw<T: Send + Sync + 'static>(&self, name: &str) -> Result<(), Error> {
        self.catalog.withdraw::<T>(&self.owner, name)
    }
    pub fn withdraw_many<T: Send + Sync + 'static>(&self, names: &[String]) -> Result<(), Error> {
        self.catalog.withdraw_many::<T>(&self.owner, names)
    }
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new(None)
    }
}
impl Catalog {
    /// Embedding dispatch configuration, never a plugin publication capability.
    pub fn with_calls(calls: crate::call::Issuer) -> Self {
        Self::new(Some(calls))
    }
    fn new(calls: Option<crate::call::Issuer>) -> Self {
        Self(Arc::new(Inner {
            id: uuid::Uuid::new_v4(),
            calls,
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
    pub(crate) fn publisher(&self, owner: Context) -> Publisher {
        Publisher {
            catalog: self.clone(),
            owner,
        }
    }
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
        crate::name(name)?;
        let kind = TypeId::of::<T>();
        let mut state = self.0.state.lock().unwrap();
        let key = (kind, name.to_owned());
        if state
            .records
            .keys()
            .any(|(ty, _, key)| *ty == kind && key == name)
        {
            return Err(Error::ContributionConflict(name.into()));
        }
        state.reserved.insert(key);
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
        self.withdraw_many::<T>(owner, &[name.to_owned()])
    }

    fn withdraw_many<T: Send + Sync + 'static>(
        &self,
        owner: &Context,
        names: &[String],
    ) -> Result<(), Error> {
        let _admission = owner.admit()?;
        let identity = owner.identity()?;
        let keys: Vec<_> = names
            .iter()
            .map(|name| (TypeId::of::<T>(), identity.scope.clone(), name.clone()))
            .collect();
        let mut state = self.0.state.lock().unwrap();
        for key in &keys {
            if let Some(record) = state.records.get(key)
                && record.owner.identity()?.activation != identity.activation
            {
                return Err(Error::ContributionConflict(key.2.clone()));
            }
        }
        let mut removed = Vec::new();
        for key in keys {
            if let Some(record) = state.records.remove(&key) {
                record.retired.cancel();
                removed.push(record);
            }
        }
        if !removed.is_empty() {
            changed(&self.0, &mut state);
        }
        drop(state);
        // A native contribution's destructor may call the public registry.
        drop(removed);
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
                    let removed: Vec<_> = state
                        .records
                        .extract_if(|_, record| record.batch == batch)
                        .map(|(_, record)| {
                            record.retired.cancel();
                            record
                        })
                        .collect();
                    if !removed.is_empty() {
                        changed(&registry, &mut state);
                    }
                    drop(state);
                    drop(removed);
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
            if state.reserved.contains(&(*kind, name.clone()))
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
            let removed: Vec<_> = state
                .records
                .extract_if(|_, record| record.batch == batch)
                .collect();
            drop(state);
            drop(removed);
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
                registration: record.batch,
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
            calls: self.0.calls.clone(),
            scope: scope.clone(),
            revision: state.revision,
            records,
        }
    }
}

pub struct Captured {
    catalog_id: uuid::Uuid,
    calls: Option<crate::call::Issuer>,
    scope: Scope,
    pub revision: u64,
    records: HashMap<(TypeId, String), Record>,
}

impl Captured {
    pub fn call_issuer(&self) -> Option<crate::call::Issuer> {
        self.calls.clone()
    }
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
                        registration: record.batch,
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
    registration: uuid::Uuid,
    retired: CancellationToken,
    pub value: Arc<T>,
    pub owner: Context,
}

impl<T> Clone for Contribution<T> {
    fn clone(&self) -> Self {
        Self {
            registration: self.registration,
            retired: self.retired.clone(),
            value: self.value.clone(),
            owner: self.owner.clone(),
        }
    }
}

impl<T> Contribution<T> {
    /// Identifies a publication, including replacement within one activation.
    pub fn registration_id(&self) -> uuid::Uuid {
        self.registration
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Reentrant(Catalog);
    impl Drop for Reentrant {
        fn drop(&mut self) {
            let _ = self.0.snapshot::<Reentrant>(&Scope::Profile);
        }
    }

    #[test]
    fn group_withdrawal_and_registration_drop_release_payloads_outside_the_registry_lock() {
        let (finished, result) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _entered = runtime.enter();
            let catalog = Catalog::default();
            let owner = Fiber::new("example", "group", Scope::Profile).unwrap();
            owner.begin_loading().unwrap();
            owner.ready().unwrap();
            owner.publish().unwrap();
            for explicit in [true, false] {
                let mut staged = Staged::default();
                for name in ["first", "second"] {
                    staged.insert(name, Reentrant(catalog.clone())).unwrap();
                }
                let registration = catalog.register(&owner.context(), staged).unwrap();
                let before = catalog.capture(&Scope::Profile).revision;
                if explicit {
                    catalog
                        .publisher(owner.context())
                        .withdraw_many::<Reentrant>(&["first".into(), "second".into()])
                        .unwrap();
                }
                drop(registration);
                let after = catalog.snapshot::<Reentrant>(&Scope::Profile);
                assert!(after.entries.is_empty());
                assert_eq!(after.revision, before + 1);
            }
            finished.send(()).unwrap();
        });
        result
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("registry deadlocked during payload cleanup");
        worker.join().unwrap();
    }
}
