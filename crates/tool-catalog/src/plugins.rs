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

use crate::{CatalogError, ToolCatalog, ToolRegistration, catalog::RegisteredTool};
use maka_plugins::{
    composition::Scope,
    contributions::{Captured, Catalog},
};
use maka_runtime::{
    tool_call::ToolRejection,
    tools::{PreparationFuture, ToolCallContext, ToolError, ToolHandler, ToolPreparer},
};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};
use tokio_util::sync::CancellationToken;

mod binding;
pub use binding::{Binding, BindingProvider, BindingRequest};

/// Constructed once during staging, not schema-compiled on every model step.
pub struct PluginTool {
    catalog: ToolCatalog,
    binding: Option<Arc<dyn BindingProvider>>,
    always_visible: bool,
}

impl PluginTool {
    pub fn new(registration: ToolRegistration) -> Result<Self, CatalogError> {
        Ok(Self {
            catalog: ToolCatalog::new([registration])?,
            binding: None,
            always_visible: false,
        })
    }
    /// Tools sharing a provider capture one handler/context pair per request.
    pub fn with_binding(mut self, binding: Arc<dyn BindingProvider>) -> Self {
        self.binding = Some(binding);
        self
    }
    pub fn definition(&self) -> &crate::ToolDefinition {
        self.catalog
            .definitions()
            .next()
            .expect("single validated plugin tool")
    }
    /// Advertise this capability without requiring a prior tool_search.
    pub fn always_visible(mut self) -> Self {
        self.always_visible = true;
        self
    }
}

#[derive(Clone)]
pub(crate) struct Source {
    catalog: Catalog,
    scope: Scope,
    ceiling: Option<Arc<BTreeSet<String>>>,
}

impl ToolCatalog {
    pub fn with_plugins(
        mut self,
        catalog: Catalog,
        scope: Scope,
        ceiling: Option<BTreeSet<String>>,
    ) -> Result<Self, CatalogError> {
        catalog
            .host_only::<PluginTool>()
            .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        for name in self.names() {
            catalog
                .reserve::<PluginTool>(&name)
                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        }
        if let Some(names) = &ceiling {
            self = self.select(|name| names.contains(name));
        }
        self.plugins = Some(Source {
            catalog,
            scope,
            ceiling: ceiling.map(Arc::new),
        });
        Ok(self)
    }

    pub fn resolve_plugins(&self) -> Result<Self, CatalogError> {
        let Some(captured) = self.capture_plugins() else {
            return Ok(self.clone());
        };
        self.resolve_captured(&captured)
    }

    pub(crate) fn capture_plugins(&self) -> Option<Captured> {
        self.plugins
            .as_ref()
            .map(|source| source.catalog.capture(&source.scope))
    }

    /// Host can use the same capture for tools and prompt contributions.
    pub fn resolve_captured(&self, captured: &Captured) -> Result<Self, CatalogError> {
        let Some(source) = &self.plugins else {
            return Ok(self.clone());
        };
        if !source.catalog.owns(captured) || captured.scope() != &source.scope {
            return Err(CatalogError::Invalid(
                "plugin capture belongs to a different Host or scope".into(),
            ));
        }
        let mut entries = (*self.entries).clone();
        let mut bytes: usize = entries.values().map(|entry| entry.bytes).sum();
        for (name, contribution) in captured.typed::<PluginTool>().entries {
            if source
                .ceiling
                .as_ref()
                .is_some_and(|ceiling| !ceiling.contains(&name))
            {
                continue;
            }
            if entries.contains_key(&name) {
                return Err(CatalogError::Invalid(format!(
                    "plugin cannot override Host tool {name}"
                )));
            }
            let registration = contribution
                .value
                .catalog
                .entries
                .values()
                .next()
                .expect("single plugin registration");
            if name != registration.registration.definition.name {
                return Err(CatalogError::Invalid(
                    "plugin registration name differs from its definition".into(),
                ));
            }
            bytes = bytes.saturating_add(registration.bytes);
            if entries.len() >= 128 || bytes > 1024 * 1024 {
                return Err(CatalogError::Invalid(
                    "plugin tool catalog exceeds request inventory limits".into(),
                ));
            }
            let mut bound = registration.registration.clone();
            let validator = registration.validator.clone();
            let entry_bytes = registration.bytes;
            let always_visible = contribution.value.always_visible;
            bound.handler = ToolHandler::Prepared(Arc::new(Guarded {
                handler: bound.handler,
                owner: contribution,
                calls: captured.call_issuer(),
            }));
            entries.insert(
                name,
                Arc::new(RegisteredTool {
                    always_visible,
                    registration: bound,
                    validator,
                    bytes: entry_bytes,
                }),
            );
        }
        Ok(Self {
            entries: Arc::new(entries),
            discovery: self.discovery,
            plugins: None,
            workspace: self.workspace.clone(),
        })
    }
}

struct Guarded {
    handler: ToolHandler,
    owner: maka_plugins::contributions::Contribution<PluginTool>,
    calls: Option<maka_plugins::call::Issuer>,
}

impl ToolPreparer for Guarded {
    fn names(&self) -> Vec<String> {
        self.handler.names()
    }
    fn prepare(
        &self,
        name: String,
        input: Value,
        context: ToolCallContext,
        cancellation: CancellationToken,
    ) -> PreparationFuture {
        if !self.owner.is_effective() {
            return Box::pin(async { Err(ToolRejection::Unavailable) });
        }
        let identity = maka_plugins::call::Identity::Agent {
            invocation: context.invocation.clone(),
            operation_id: Some(context.operation_id.clone()),
        };
        let calls = self.calls.clone();
        let preparation = self.handler.prepare(name, input, context, cancellation);
        let owner = self.owner.clone();
        Box::pin(async move {
            let scope_owner = owner.owner.clone();
            let prepared = preparation.await?.guarded(move || {
                owner
                    .admit()
                    .map_err(|error| ToolError::Failed(error.to_string()))
            });
            Ok(match calls {
                None => prepared,
                Some(issuer) => prepared.map_future(move |operation, stop| {
                    Box::pin(async move {
                        let result = issuer.run(identity, stop, operation).await;
                        if let Err(ToolError::CleanupUnconfirmed(reason)) = &result {
                            scope_owner.cleanup_failed(reason.clone());
                        }
                        result
                    })
                }),
            })
        })
    }
}
