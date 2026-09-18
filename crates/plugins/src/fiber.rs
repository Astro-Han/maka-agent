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

mod context;
mod effect;
mod shutdown;
mod task;

pub use context::{CallGuard, Context};
pub use effect::Effect;

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::{
    runtime::Handle,
    sync::{Notify, watch},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{Error, composition::Scope, identifier, name};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Pending,
    Loading,
    Active,
    Unloading,
    Failed,
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub package_id: String,
    pub entry_id: String,
    pub scope: Scope,
    pub activation: String,
    /// Process-local inspection sequence, never a durable or routing identity.
    pub generation: u64,
}

/// The unique instance owner. Contexts borrow it weakly; admitted calls keep their
/// resources alive until settlement. Drop starts, but cannot acknowledge, cleanup.
pub struct Fiber {
    inner: Arc<Inner>,
}

struct Inner {
    identity: Identity,
    state: Mutex<State>,
    runtime: Handle,
    stopping: CancellationToken,
    changed: watch::Sender<()>,
    settled: Notify,
}

struct State {
    phase: Phase,
    effective: bool,
    calls: usize,
    effects: BTreeMap<u64, Effect>,
    next_effect: u64,
    children: Vec<Fiber>,
    failures: Vec<String>,
}

impl Fiber {
    pub fn new(package: &str, entry: &str, scope: Scope) -> Result<Self, Error> {
        Self::observed(package, entry, scope, watch::channel(()).0)
    }

    pub(crate) fn observed(
        package: &str,
        entry: &str,
        scope: Scope,
        changed: watch::Sender<()>,
    ) -> Result<Self, Error> {
        identifier(package)?;
        identifier(entry)?;
        if let Scope::Session(session) = &scope {
            name(session)?;
        }
        let runtime = Handle::try_current()
            .map_err(|_| Error::Lifecycle("Fiber requires a running executor"))?;
        static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let generation = GENERATION
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |current| (current < (1 << 53) - 1).then_some(current + 1),
            )
            .map_err(|_| Error::Lifecycle("inspection generation exhausted"))?;
        Ok(Self {
            inner: Arc::new(Inner {
                identity: Identity {
                    package_id: package.into(),
                    entry_id: entry.into(),
                    scope,
                    activation: uuid::Uuid::new_v4().to_string(),
                    generation,
                },
                state: Mutex::new(State {
                    phase: Phase::Pending,
                    effective: false,
                    calls: 0,
                    effects: BTreeMap::new(),
                    next_effect: 0,
                    children: Vec::new(),
                    failures: Vec::new(),
                }),
                runtime,
                stopping: CancellationToken::new(),
                changed,
                settled: Notify::new(),
            }),
        })
    }

    pub fn identity(&self) -> &Identity {
        &self.inner.identity
    }
    pub fn phase(&self) -> Phase {
        self.inner.state.lock().unwrap().phase
    }
    pub fn context(&self) -> Context {
        Context {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub fn begin_loading(&self) -> Result<(), Error> {
        self.transition(Phase::Pending, Phase::Loading)
    }

    /// Services may become ready before typed contributions are published.
    pub fn ready(&self) -> Result<(), Error> {
        self.transition(Phase::Loading, Phase::Active)
    }

    pub fn publish(&self) -> Result<(), Error> {
        self.context().publish()
    }

    /// Failed ownership transfer returns the child to its previous owner.
    pub fn own_child(&self, child: Fiber) -> Result<(), Fiber> {
        self.context().own_child(child)
    }

    pub fn retire(&self) {
        self.inner.retire();
    }

    pub async fn shutdown(&self, deadline: Instant) -> Result<(), Error> {
        self.context().shutdown(deadline).await
    }

    fn transition(&self, from: Phase, to: Phase) -> Result<(), Error> {
        self.context().transition(from, to)
    }
}

impl Drop for Fiber {
    fn drop(&mut self) {
        self.inner.retire();
    }
}
