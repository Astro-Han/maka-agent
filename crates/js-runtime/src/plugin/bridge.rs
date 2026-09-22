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

use super::{Result, failed};
use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// A module-scoped Host capability router. Host validates instance identity and
/// invocation authority; the VM transports only encodable values.
pub trait Bridge: Send + Sync {
    fn max_output_bytes(&self, _method: &str) -> usize {
        1024 * 1024
    }
    /// Encoded payload allowance; capability implementations validate their
    /// decoded input separately. The VM-wide in-flight budget still applies.
    fn max_input_bytes(&self, _method: &str) -> usize {
        1024 * 1024
    }
    fn call(&self, method: String, input: Value) -> BoxFuture<'static, Result<Value>>;
}

struct Binding {
    bridge: Arc<dyn Bridge>,
    closing: CancellationToken,
}
impl Drop for Binding {
    fn drop(&mut self) {
        self.closing.cancel();
    }
}
pub(super) struct Bindings {
    entries: BTreeMap<String, Binding>,
    calls: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
}
impl Default for Bindings {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            calls: Arc::new(Semaphore::new(128)),
            bytes: Arc::new(Semaphore::new(32 * 1024 * 1024)),
        }
    }
}
impl Bindings {
    pub fn insert(&mut self, id: u64, bridge: Arc<dyn Bridge>) {
        self.entries.insert(
            id.to_string(),
            Binding {
                bridge,
                closing: CancellationToken::new(),
            },
        );
    }
    pub fn remove(&mut self, id: u64) {
        self.entries.remove(&id.to_string());
    }
}

#[op2]
#[serde]
async fn op_maka_plugin(
    state: Rc<RefCell<OpState>>,
    #[string] id: String,
    #[string] method: String,
    #[serde] input: serde_json::Value,
) -> std::result::Result<serde_json::Value, JsErrorBox> {
    let (bridge, closing, _call, _bytes) = {
        let state = state.borrow();
        let bindings = state.borrow::<Bindings>();
        let binding = bindings
            .entries
            .get(&id)
            .ok_or_else(|| JsErrorBox::generic("plugin instance is retired"))?;
        if method.is_empty() || method.len() > 128 {
            return Err(JsErrorBox::generic("invalid Host method"));
        }
        let size = serde_json::to_vec(&input)
            .map_err(|error| JsErrorBox::generic(error.to_string()))?
            .len();
        if size
            > binding
                .bridge
                .max_input_bytes(&method)
                .min(32 * 1024 * 1024)
        {
            return Err(JsErrorBox::generic(
                "Host call exceeds its input byte limit",
            ));
        }
        let call = bindings
            .calls
            .clone()
            .try_acquire_owned()
            .map_err(|_| JsErrorBox::generic("Host call capacity exhausted"))?;
        let bytes = bindings
            .bytes
            .clone()
            .try_acquire_many_owned(size.max(1) as u32)
            .map_err(|_| JsErrorBox::generic("Host input capacity exhausted"))?;
        (binding.bridge.clone(), binding.closing.clone(), call, bytes)
    };
    let output_limit = bridge.max_output_bytes(&method).min(32 * 1024 * 1024);
    let output = tokio::select! {
        biased;
        _ = closing.cancelled() => Err(failed("plugin instance is retired")),
        output = bridge.call(method, input) => output,
    }
    .map_err(|error| JsErrorBox::generic(error.to_string()))?;
    if serde_json::to_vec(&output)
        .map_err(|error| JsErrorBox::generic(error.to_string()))?
        .len()
        > output_limit
    {
        return Err(JsErrorBox::generic(
            "Host result exceeds its output byte limit",
        ));
    }
    // Even an immediately ready Host operation must return control to the VM
    // owner. Otherwise an async JS loop can starve queued revocations/cleanup.
    tokio::task::yield_now().await;
    Ok(output)
}

deno_core::extension!(maka_plugins, ops = [op_maka_plugin]);
