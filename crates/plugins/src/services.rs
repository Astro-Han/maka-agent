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

pub mod method;

use crate::{
    Error, Registration,
    composition::{Entry, Isolation},
    fiber::{CallGuard, Context, Effect},
};
use serde_json::Value;
use std::{
    any::Any,
    collections::BTreeMap,
    ops::Deref,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::watch;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Label {
    Shared(String),
    Named(String),
    Private(uuid::Uuid),
}

type ServiceKey = (String, Label);

struct Record {
    active: Arc<AtomicBool>,
    token: uuid::Uuid,
    provider: Context,
    value: Arc<dyn Any + Send + Sync>,
}

struct Inner {
    records: Mutex<BTreeMap<ServiceKey, Record>>,
    changed: watch::Sender<()>,
}

/// Services become readable when their provider is active, independently of
/// contribution publication. The owning Fiber holds the revocation Effect.
#[derive(Clone)]
pub struct Services(Arc<Inner>);

impl Default for Services {
    fn default() -> Self {
        Self(Arc::new(Inner {
            records: Mutex::new(BTreeMap::new()),
            changed: watch::channel(()).0,
        }))
    }
}

impl Services {
    pub fn view(&self) -> ServiceView {
        ServiceView {
            services: self.clone(),
            labels: BTreeMap::new(),
            intercepts: BTreeMap::new(),
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.0.changed.subscribe()
    }

    /// Activation changes readiness without changing a registration's identity.
    pub(crate) fn readiness_changed(&self) {
        self.0.changed.send_replace(());
    }
}

/// Derived views carry routing/configuration, never ownership of registrations.
#[derive(Clone)]
pub struct ServiceView {
    services: Services,
    labels: BTreeMap<String, Label>,
    intercepts: BTreeMap<String, Vec<Value>>,
}

/// Plugin-facing services bind both registration ownership and consumer
/// lifetime. Composition routing and arbitrary owner selection stay with Host.
#[derive(Clone)]
pub struct BoundServices {
    view: ServiceView,
    owner: Context,
}
impl BoundServices {
    pub fn provide<T: Send + Sync + 'static>(
        &self,
        name: &str,
        value: Arc<T>,
    ) -> Result<(), Error> {
        self.register(name, value).map(Registration::retain)
    }
    pub fn register<T: Send + Sync + 'static>(
        &self,
        name: &str,
        value: Arc<T>,
    ) -> Result<Registration, Error> {
        self.view.register(&self.owner, name, value)
    }
    pub fn get<T: Send + Sync + 'static>(&self, name: &str) -> Result<Option<Service<T>>, Error> {
        let _lease = self.owner.resource_call()?;
        Ok(self.view.get(name)?.map(|mut service| {
            service.consumer = Some(self.owner.clone());
            service
        }))
    }
}

impl ServiceView {
    pub(crate) fn bind(&self, owner: Context) -> BoundServices {
        BoundServices {
            view: self.clone(),
            owner,
        }
    }
    pub fn derive(&self, entry: &Entry) -> Result<Self, Error> {
        let mut view = self.clone();
        for (name, isolation) in &entry.isolate {
            validate_name(name)?;
            let label = match isolation {
                Isolation::Private(true) => Label::Private(uuid::Uuid::new_v4()),
                Isolation::Named(label) => Label::Named(label.clone()),
                Isolation::Private(false) => {
                    return Err(Error::Invalid("isolate must be true or a label".into()));
                }
            };
            view.labels.insert(name.clone(), label);
        }
        for (name, config) in &entry.intercept {
            validate_name(name)?;
            view.intercepts
                .entry(name.clone())
                .or_default()
                .push(config.clone());
        }
        Ok(view)
    }

    pub fn intercepts(&self, name: &str) -> &[Value] {
        self.intercepts
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn provide<T: Send + Sync + 'static>(
        &self,
        owner: &Context,
        name: &str,
        value: Arc<T>,
    ) -> Result<(), Error> {
        self.register(owner, name, value).map(Registration::retain)
    }

    pub fn register<T: Send + Sync + 'static>(
        &self,
        owner: &Context,
        name: &str,
        value: Arc<T>,
    ) -> Result<Registration, Error> {
        let _lease = owner.resource_call()?;
        let key = self.key(name)?;
        let token = uuid::Uuid::new_v4();
        let active = Arc::new(AtomicBool::new(true));
        {
            let mut records = self.services.0.records.lock().unwrap();
            if records.contains_key(&key) {
                return Err(Error::DuplicateService(name.into()));
            }
            records.insert(
                key.clone(),
                Record {
                    active,
                    token,
                    provider: owner.clone(),
                    value,
                },
            );
        }
        let registry = Arc::downgrade(&self.services.0);
        let effect = Effect::new(
            format!("service {name}"),
            move || revoke(&registry, &key, token),
            || async { Ok(()) },
        );
        let registration = Registration::new(owner, effect)?;
        self.services.readiness_changed();
        Ok(registration)
    }

    pub fn get<T: Send + Sync + 'static>(&self, name: &str) -> Result<Option<Service<T>>, Error> {
        let key = self.key(name)?;
        let records = self.services.0.records.lock().unwrap();
        let Some(record) = records
            .get(&key)
            .filter(|record| record.provider.is_ready())
        else {
            return Ok(None);
        };
        Ok(Some(Service {
            consumer: None,
            active: record.active.clone(),
            value: record
                .value
                .clone()
                .downcast()
                .map_err(|_| Error::ServiceType(name.into()))?,
            provider: record.provider.clone(),
        }))
    }

    /// A replaced registration changes dependency identity, even when the same
    /// active provider registers the same name again.
    pub fn dependencies<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<Option<BTreeMap<String, uuid::Uuid>>, Error> {
        let records = self.services.0.records.lock().unwrap();
        let mut dependencies = BTreeMap::new();
        for name in names {
            let key = self.key(name)?;
            let Some(record) = records
                .get(&key)
                .filter(|record| record.provider.is_ready())
            else {
                return Ok(None);
            };
            dependencies.insert(name.into(), record.token);
        }
        Ok(Some(dependencies))
    }

    fn key(&self, name: &str) -> Result<ServiceKey, Error> {
        validate_name(name)?;
        Ok((
            name.into(),
            self.labels
                .get(name)
                .cloned()
                .unwrap_or_else(|| Label::Shared(name.into())),
        ))
    }
}

/// A captured handle never switches to another provider after retirement.
pub struct Service<T> {
    consumer: Option<Context>,
    active: Arc<AtomicBool>,
    value: Arc<T>,
    provider: Context,
}

impl<T> Clone for Service<T> {
    fn clone(&self) -> Self {
        Self {
            consumer: self.consumer.clone(),
            active: self.active.clone(),
            value: self.value.clone(),
            provider: self.provider.clone(),
        }
    }
}

impl<T> Service<T> {
    pub fn acquire(&self) -> Result<ServiceCall<T>, Error> {
        let consumer = self
            .consumer
            .as_ref()
            .map(Context::resource_call)
            .transpose()?;
        let call = self.provider.service_call()?;
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::Retired);
        }
        Ok(ServiceCall {
            value: self.value.clone(),
            _call: call,
            _consumer: consumer,
        })
    }
}

pub struct ServiceCall<T> {
    _consumer: Option<CallGuard>,
    value: Arc<T>,
    _call: CallGuard,
}

impl<T> Deref for ServiceCall<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

fn revoke(registry: &Weak<Inner>, key: &ServiceKey, token: uuid::Uuid) {
    if let Some(registry) = registry.upgrade() {
        let mut records = registry.records.lock().unwrap();
        if records.get(key).is_some_and(|record| record.token == token) {
            if let Some(record) = records.remove(key) {
                record.active.store(false, Ordering::SeqCst);
            }
            registry.changed.send_replace(());
        }
    }
}

pub(crate) fn validate_name(name: &str) -> Result<(), Error> {
    if name.len() > 256
        || !name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
    {
        return Err(Error::Invalid(format!("invalid service name: {name:?}")));
    }
    Ok(())
}
