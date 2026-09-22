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

use crate::{ModelError, ProviderKind, sdk};
use futures_util::future::BoxFuture;
use maka_js_runtime::trusted::TrustedRuntime;
use maka_plugins::{
    composition::Scope,
    contributions::{Captured, Catalog, Staged},
    fiber::Fiber,
    kernel::{Plugin, PluginContext},
    model::{Adapter, Binding},
};
use std::sync::Arc;

pub const ID: &str = "maka.models";

/// Distribution defaults only. Selection and retirement use the same catalog
/// as external adapters, with ordinary Session-over-profile shadowing.
pub struct Builtin(pub TrustedRuntime);
impl Builtin {
    pub fn stage(&self) -> Result<Staged, maka_plugins::Error> {
        let mut staged = Staged::default();
        staged.insert(
            "responses",
            Adapter {
                provider: Arc::new(maka_responses::Adapter::default()),
            },
        )?;
        let sdk = Arc::new(sdk::Adapter(self.0.clone()));
        for name in ["chat-completions", "anthropic-messages"] {
            staged.insert(
                name,
                Adapter {
                    provider: sdk.clone(),
                },
            )?;
        }
        Ok(staged)
    }
}
impl Plugin for Builtin {
    fn activate(
        &self,
        _: PluginContext,
        _: serde_json::Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        let result = self.stage().map_err(|error| error.to_string());
        Box::pin(async { result })
    }
}
pub(crate) fn standalone(runtime: TrustedRuntime) -> Result<(Catalog, Arc<Fiber>), ModelError> {
    let setup = || {
        let catalog = Catalog::default();
        let owner = Fiber::new(ID, ID, Scope::Profile)?;
        owner.begin_loading()?;
        owner.ready()?;
        catalog.publish(&owner, Builtin(runtime).stage()?)?;
        Ok::<_, maka_plugins::Error>((catalog, Arc::new(owner)))
    };
    setup().map_err(|error| ModelError::Adapter(error.to_string()))
}

pub fn name(kind: &ProviderKind) -> &str {
    match kind {
        ProviderKind::OpenaiResponses | ProviderKind::OpenResponses(_) => "responses",
        ProviderKind::Anthropic => "anthropic-messages",
        ProviderKind::OpenaiChat | ProviderKind::OpenaiCompatible { .. } => "chat-completions",
    }
}
pub fn resolve(captured: &Captured, name: &str) -> Result<Binding, ModelError> {
    captured
        .typed::<Adapter>()
        .entries
        .remove(name)
        .map(Binding::new)
        .ok_or_else(|| ModelError::Adapter(format!("model adapter is unavailable: {name}")))
}
