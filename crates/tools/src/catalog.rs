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

use crate::{PreparationFuture, ToolCallContext};
use jsonschema::Validator;
use maka_runtime::tool_call::ToolRejection;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use tokio_util::sync::CancellationToken;

pub use maka_runtime::{
    execution::ToolMode,
    tools::{ToolDefinition, ToolNesting, ToolRegistration, ToolSemantics},
};

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("invalid tool catalog: {0}")]
    Invalid(String),
    #[error("invalid tool schema: {0}")]
    Schema(String),
}

pub(crate) struct RegisteredTool {
    pub(crate) registration: ToolRegistration,
    pub(crate) validator: Arc<Validator>,
    pub(crate) bytes: usize,
}

#[derive(Clone, Default)]
pub struct ToolCatalog {
    pub(crate) entries: Arc<BTreeMap<String, Arc<RegisteredTool>>>,
    pub(super) discovery: bool,
    pub(crate) plugins: Option<crate::plugins::Source>,
}

impl ToolCatalog {
    pub fn new(
        registrations: impl IntoIterator<Item = ToolRegistration>,
    ) -> Result<Self, CatalogError> {
        let mut entries = BTreeMap::new();
        let mut bytes = 0usize;
        for registration in registrations {
            if registration.semantics == ToolSemantics::FinishTurn
                && registration.nesting != ToolNesting::DirectOnly
            {
                return Err(CatalogError::Invalid(
                    "a finishing tool must be direct-only".into(),
                ));
            }
            let definition = &registration.definition;
            if entries.len() == 128
                || definition.name.is_empty()
                || definition.name.len() > 128
                || matches!(
                    definition.name.as_str(),
                    "exec" | "invalid" | "tool_search" | "maka_tool_search"
                )
                || entries.contains_key(&definition.name)
                || !registration.handler.names().contains(&definition.name)
            {
                return Err(CatalogError::Invalid(
                    "duplicate, reserved, absent or excessive tool name".into(),
                ));
            }
            let definition_bytes = definition
                .name
                .len()
                .saturating_add(definition.description.len())
                .saturating_add(
                    serde_json::to_vec(&definition.input_schema)
                        .map_err(|e| CatalogError::Schema(e.to_string()))?
                        .len(),
                );
            bytes = bytes.saturating_add(definition_bytes);
            if bytes > 1024 * 1024 {
                return Err(CatalogError::Invalid("definition budget exceeded".into()));
            }
            // Matches Maka's JSON-schema tool path: no format assertions,
            // defaults/coercion or fetching file/network references.
            let validator = jsonschema::options()
                .offline()
                .should_validate_formats(false)
                .build(&definition.input_schema)
                .map_err(|e| CatalogError::Schema(e.to_string()))?;
            entries.insert(
                definition.name.clone(),
                Arc::new(RegisteredTool {
                    registration,
                    validator: Arc::new(validator),
                    bytes: definition_bytes,
                }),
            );
        }
        Ok(Self {
            entries: Arc::new(entries),
            discovery: false,
            plugins: None,
        })
    }

    /// Defer non-core schemas until discovered within this Run.
    pub fn with_discovery(mut self) -> Self {
        self.discovery = true;
        self
    }

    pub(super) fn select(&self, keep: impl Fn(&str) -> bool) -> Self {
        Self {
            entries: Arc::new(
                self.entries
                    .iter()
                    .filter(|(name, _)| keep(name))
                    .map(|(name, entry)| (name.clone(), entry.clone()))
                    .collect(),
            ),
            discovery: self.discovery,
            plugins: self.plugins.clone(),
        }
    }

    pub fn definitions(&self) -> impl Iterator<Item = &ToolDefinition> {
        self.entries
            .values()
            .map(|entry| &entry.registration.definition)
    }

    pub fn nested(&self) -> Self {
        Self {
            entries: Arc::new(
                self.entries
                    .iter()
                    .filter(|(_, entry)| entry.registration.nesting == ToolNesting::Nestable)
                    .map(|(name, entry)| (name.clone(), entry.clone()))
                    .collect(),
            ),
            discovery: self.discovery,
            plugins: self.plugins.clone(),
        }
    }

    pub(super) fn direct_only(&self) -> Self {
        self.select(|name| self.entries[name].registration.nesting == ToolNesting::DirectOnly)
    }

    /// Stable request-surface identity; process-local handler addresses are excluded.
    pub fn digest(&self) -> String {
        let entries: Vec<_> = self
            .entries
            .values()
            .map(|entry| {
                let r = &entry.registration;
                (
                    &r.definition,
                    r.nesting == ToolNesting::Nestable,
                    r.semantics != ToolSemantics::Parallel,
                )
            })
            .collect();
        let finishing: Vec<_> = self
            .entries
            .values()
            .filter(|entry| entry.registration.semantics == ToolSemantics::FinishTurn)
            .map(|entry| &entry.registration.definition.name)
            .collect();
        // Preserve the existing durable identity when no new semantic is used.
        let bytes = if finishing.is_empty() {
            serde_json::to_vec(&(self.discovery, entries))
        } else {
            serde_json::to_vec(&(self.discovery, entries, finishing))
        }
        .expect("tool definitions are JSON");
        maka_runtime::artifact::content_digest(&bytes)
    }

    pub fn semantics(&self, name: &str) -> Result<ToolSemantics, ToolRejection> {
        self.entries
            .get(name)
            .map(|entry| entry.registration.semantics)
            .ok_or(ToolRejection::Unavailable)
    }

    pub fn validate(&self, name: &str, input: &Value) -> Result<(), ToolRejection> {
        let entry = self.entries.get(name).ok_or(ToolRejection::Unavailable)?;
        if !entry.validator.is_valid(input) {
            return Err(ToolRejection::InvalidInput {
                message: "arguments do not match the declared schema".into(),
            });
        }
        Ok(())
    }
}

impl ToolCatalog {
    pub fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn prepare(
        &self,
        name: String,
        input: Value,
        context: ToolCallContext,
        cancellation: CancellationToken,
    ) -> PreparationFuture {
        if cancellation.is_cancelled() {
            return Box::pin(async { Err(ToolRejection::Cancelled) });
        }
        if let Err(reason) = self.validate(&name, &input) {
            return Box::pin(async move { Err(reason) });
        }
        let preparation = self.entries[&name].registration.handler.prepare(
            name,
            input,
            context,
            cancellation.clone(),
        );
        Box::pin(async move {
            let effect = preparation.await?;
            if cancellation.is_cancelled() {
                return Err(ToolRejection::Cancelled);
            }
            Ok(effect)
        })
    }
}
