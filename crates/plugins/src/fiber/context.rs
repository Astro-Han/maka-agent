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

use std::sync::{Arc, Weak};
use tokio_util::sync::CancellationToken;

use super::{Effect, Fiber, Identity, Inner, Phase};
use crate::Error;

#[derive(Clone)]
pub struct Context {
    pub(super) inner: Weak<Inner>,
}

enum Admission {
    Resource,
    Service,
    Effective,
}

impl Context {
    pub(crate) fn phase(&self) -> Phase {
        self.inner
            .upgrade()
            .map(|inner| inner.state.lock().unwrap().phase)
            .unwrap_or(Phase::Disposed)
    }

    pub(crate) fn publish(&self) -> Result<(), Error> {
        let inner = self.inner.upgrade().ok_or(Error::Retired)?;
        let mut state = inner.state.lock().unwrap();
        if state.phase != Phase::Active {
            return Err(Error::Retired);
        }
        state.effective = true;
        inner.changed.send_replace(());
        Ok(())
    }

    pub(crate) fn transition(&self, from: Phase, to: Phase) -> Result<(), Error> {
        let inner = self.inner.upgrade().ok_or(Error::Retired)?;
        let mut state = inner.state.lock().unwrap();
        if state.phase != from {
            return Err(Error::Lifecycle("unexpected instance phase"));
        }
        state.phase = to;
        inner.changed.send_replace(());
        Ok(())
    }

    /// Request retirement; the Fiber remains the sole cleanup owner.
    pub fn retire(&self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.retire();
        }
    }

    /// Fence this instance when an owned Host resource cannot confirm cleanup.
    pub fn cleanup_failed(&self, reason: String) {
        if let Some(inner) = self.inner.upgrade() {
            {
                let mut state = inner.state.lock().unwrap();
                if state.failures.len() < 64 {
                    state.failures.push(reason.chars().take(2048).collect());
                }
            }
            inner.retire();
        }
    }

    pub async fn shutdown(&self, deadline: tokio::time::Instant) -> Result<(), Error> {
        let Some(inner) = self.inner.upgrade() else {
            return Ok(());
        };
        inner.retire();
        let mut changed = inner.changed.subscribe();
        let wait = async {
            loop {
                {
                    let state = inner.state.lock().unwrap();
                    match state.phase {
                        Phase::Disposed => return Ok(()),
                        Phase::Failed => return Err(Error::Cleanup(state.failures.clone())),
                        _ => {}
                    }
                }
                changed.changed().await.map_err(|_| Error::CleanupPending)?;
            }
        };
        tokio::time::timeout_at(deadline, wait)
            .await
            .map_err(|_| Error::CleanupPending)?
    }

    pub(crate) fn own_child(&self, child: Fiber) -> Result<(), Fiber> {
        let Some(inner) = self.inner.upgrade() else {
            return Err(child);
        };
        let mut state = inner.state.lock().unwrap();
        if !matches!(state.phase, Phase::Pending | Phase::Loading | Phase::Active) {
            return Err(child);
        }
        state
            .children
            .retain(|child| child.phase() != Phase::Disposed);
        state.children.push(child);
        Ok(())
    }

    pub fn identity(&self) -> Result<Identity, Error> {
        Ok(self.inner.upgrade().ok_or(Error::Retired)?.identity.clone())
    }

    pub fn active_calls(&self) -> usize {
        self.inner
            .upgrade()
            .map_or(0, |inner| inner.state.lock().unwrap().calls)
    }

    pub(crate) fn effect_labels(&self) -> Vec<String> {
        self.inner.upgrade().map_or_else(Vec::new, |inner| {
            inner
                .state
                .lock()
                .unwrap()
                .effects
                .values()
                .map(|effect| effect.label().chars().take(512).collect())
                .collect()
        })
    }

    pub fn stopping(&self) -> Result<CancellationToken, Error> {
        Ok(self.inner.upgrade().ok_or(Error::Retired)?.stopping.clone())
    }

    pub fn is_ready(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| inner.state.lock().unwrap().phase == Phase::Active)
    }

    pub(crate) fn cleanup_failure(&self) -> Option<String> {
        let inner = self.inner.upgrade()?;
        let state = inner.state.lock().unwrap();
        (!state.failures.is_empty()).then(|| state.failures.join("; "))
    }

    pub fn is_effective(&self) -> bool {
        self.inner.upgrade().is_some_and(|inner| {
            let state = inner.state.lock().unwrap();
            state.phase == Phase::Active && state.effective
        })
    }

    /// The creator retains ownership on rejection and must finish resource cleanup.
    pub fn own(&self, effect: Effect) -> Result<(), Effect> {
        self.own_effect(effect).map(|_| ())
    }

    pub(crate) fn own_effect(&self, effect: Effect) -> Result<u64, Effect> {
        let Some(inner) = self.inner.upgrade() else {
            return Err(effect);
        };
        let mut state = inner.state.lock().unwrap();
        if !matches!(state.phase, Phase::Loading | Phase::Active) {
            return Err(effect);
        }
        let Some(id) = state.next_effect.checked_add(1) else {
            return Err(effect);
        };
        state.next_effect = id;
        state.effects.insert(id, effect);
        Ok(id)
    }

    pub(super) fn task_completed(&self, id: u64) {
        if let Some(inner) = self.inner.upgrade() {
            let effect = inner.state.lock().unwrap().effects.remove(&id);
            if let Some(effect) = effect {
                effect.completed();
            }
            inner.changed.send_replace(());
        }
    }

    pub(crate) fn take_effect(&self, id: u64) -> Option<Effect> {
        self.inner
            .upgrade()?
            .state
            .lock()
            .unwrap()
            .effects
            .remove(&id)
    }

    /// Admission and retirement serialize on the same short-held lifecycle lock.
    pub fn admit(&self) -> Result<CallGuard, Error> {
        self.acquire(Admission::Effective)
    }

    pub(crate) fn service_call(&self) -> Result<CallGuard, Error> {
        self.acquire(Admission::Service)
    }

    /// Storage/configuration access during initialization is not business
    /// execution admission. The Host still validates the resource's authority.
    pub fn resource_call(&self) -> Result<CallGuard, Error> {
        self.acquire(Admission::Resource)
    }

    fn acquire(&self, admission: Admission) -> Result<CallGuard, Error> {
        let inner = self.inner.upgrade().ok_or(Error::Retired)?;
        {
            let mut state = inner.state.lock().unwrap();
            let permitted = match admission {
                Admission::Resource => matches!(state.phase, Phase::Loading | Phase::Active),
                Admission::Service => state.phase == Phase::Active,
                Admission::Effective => state.phase == Phase::Active && state.effective,
            };
            if !permitted {
                return Err(Error::Retired);
            }
            state.calls += 1;
        }
        Ok(CallGuard { inner })
    }

    /// Business loops wait here, not in initialization which precedes publication.
    pub async fn effective(&self) -> Result<(), Error> {
        let inner = self.inner.upgrade().ok_or(Error::Retired)?;
        let mut changed = inner.changed.subscribe();
        loop {
            {
                let state = inner.state.lock().unwrap();
                if state.phase == Phase::Active && state.effective {
                    return Ok(());
                }
                if matches!(
                    state.phase,
                    Phase::Unloading | Phase::Failed | Phase::Disposed
                ) {
                    return Err(Error::Retired);
                }
            }
            changed.changed().await.map_err(|_| Error::Retired)?;
        }
    }
}

/// This guard must survive through settlement, not merely through dispatch.
pub struct CallGuard {
    inner: Arc<Inner>,
}

impl CallGuard {
    pub fn identity(&self) -> &Identity {
        &self.inner.identity
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();
        state.calls -= 1;
        if state.calls == 0 {
            self.inner.settled.notify_one();
        }
    }
}
