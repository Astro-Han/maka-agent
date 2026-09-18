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

use super::{Inner, Limits, Result, Vm, failed};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::Semaphore;

/// Lazy execution placement. Weak references never keep an unused VM alive.
/// Dedicated capacity is held through worker teardown, not just handle release.
pub struct Pool {
    limits: Limits,
    dedicated: Arc<Semaphore>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    shared: Weak<Inner>,
    dedicated: BTreeMap<String, Weak<Inner>>,
}
impl Pool {
    pub fn new(limits: Limits, dedicated_limit: usize) -> Result<Self> {
        if dedicated_limit > 64 {
            return Err(failed("dedicated VM limit exceeds 64"));
        }
        Ok(Self {
            limits,
            dedicated: Arc::new(Semaphore::new(dedicated_limit)),
            state: Mutex::new(State::default()),
        })
    }

    pub fn shared(&self) -> Result<Vm> {
        let mut state = self.state.lock().unwrap();
        if let Some(vm) = alive(&state.shared) {
            return Ok(vm);
        }
        let vm = Vm::new(self.limits.clone())?;
        state.shared = Arc::downgrade(&vm.0);
        Ok(vm)
    }

    /// The key identifies a loaded package generation, not an Entry or request.
    pub fn dedicated(&self, generation: &str) -> Result<Vm> {
        if generation.is_empty() || generation.len() > 1024 {
            return Err(failed("invalid package generation"));
        }
        let mut state = self.state.lock().unwrap();
        if let Some(vm) = state.dedicated.get(generation).and_then(alive) {
            return Ok(vm);
        }
        state.dedicated.retain(|_, vm| vm.strong_count() != 0);
        let reservation = self
            .dedicated
            .clone()
            .try_acquire_owned()
            .map_err(|_| failed("dedicated VM capacity exhausted"))?;
        let vm = Vm::start(self.limits.clone(), Some(reservation))?;
        state
            .dedicated
            .insert(generation.into(), Arc::downgrade(&vm.0));
        Ok(vm)
    }
}
fn alive(inner: &Weak<Inner>) -> Option<Vm> {
    inner.upgrade().map(Vm).filter(|vm| !vm.is_terminated())
}
