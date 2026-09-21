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

use super::{ModelMetadata, ProviderFacts, claude_vision, wire_limit};
use crate::{ConfigError, Result};
use maka_runtime::{
    configuration::{ConnectionCatalogEntry, ModelInfo, ModelModality, validation},
    execution::ThinkingLevel,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelCatalogEntry {
    /// Effective facts for execution; the catalog's metadata remains the wire authority.
    #[serde(skip)]
    pub capabilities: maka_runtime::configuration::ModelCapabilities,
    pub id: String,
    pub can_use_as_chat_default: bool,
    pub is_default: bool,
    pub supports_vision: bool,
    pub thinking_levels: Vec<ThinkingLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_supports_vision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_input_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_threshold: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_cutoff: Option<String>,
}

impl ModelCatalogEntry {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            can_use_as_chat_default: true,
            ..Self::default()
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validation::text(&self.id, 512, true).map_err(ConfigError::Invalid)?;
        for (value, max) in [
            (self.display_name.as_deref(), 512),
            (self.description.as_deref(), 2048),
            (self.knowledge_cutoff.as_deref(), 2048),
        ] {
            if let Some(value) = value {
                validation::text(value, max, false).map_err(ConfigError::Invalid)?;
            }
        }
        for value in [self.context_window, self.input_limit]
            .into_iter()
            .flatten()
        {
            validation::revision(value, true).map_err(ConfigError::Invalid)?;
        }
        for (index, level) in self.thinking_levels.iter().enumerate() {
            if self.thinking_levels[..index].contains(level) {
                return Err(ConfigError::Invalid(
                    "duplicate catalog thinking level".into(),
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn resolve(
    row: &ConnectionCatalogEntry,
    provider: &ProviderFacts,
    model: &ModelInfo,
    default: Option<&str>,
) -> ModelCatalogEntry {
    let id = model.id.trim();
    let known = provider.models.get(id);
    let empty = ModelMetadata::default();
    let metadata = known.map(|facts| &facts.metadata).unwrap_or(&empty);
    let fallback = metadata.capabilities.unwrap_or_default();
    let profile = row
        .model_overrides
        .as_ref()
        .and_then(|profiles| profiles.get(id));
    let mut result = known
        .map(|facts| facts.entry.clone())
        .unwrap_or_else(|| ModelCatalogEntry {
            supports_vision: claude_vision(&row.provider_type, id),
            ..ModelCatalogEntry::new(id)
        });
    result.is_default = default == Some(id);
    result.default_supports_vision = Some(
        model
            .capabilities
            .and_then(|caps| caps.vision)
            .or(fallback.vision)
            .unwrap_or_else(|| claude_vision(&row.provider_type, id)),
    );
    result.default_context_window = model.context_window.or(metadata.context_window);
    result.default_input_limit = model.input_limit.or(metadata.input_limit);
    let effective;
    let model = if let Some(profile) = profile {
        effective = profile.apply(model);
        &effective
    } else {
        model
    };
    if let Some(name) = model
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != id)
    {
        result.display_name = Some(wire_limit(name, 512));
    }
    if let Some(text) = model
        .description
        .as_deref()
        .or(metadata.description.as_deref())
    {
        result.description = Some(wire_limit(text, 2048));
    }
    result.context_window = model
        .context_window
        .or(metadata.context_window)
        .or(result.context_window);
    result.input_limit = model
        .input_limit
        .or(metadata.input_limit)
        .or(result.input_limit);
    if let Some(text) = model
        .knowledge_cutoff
        .as_ref()
        .or(metadata.knowledge_cutoff.as_ref())
    {
        result.knowledge_cutoff = Some(text.clone());
    }
    let capabilities = model.capabilities.unwrap_or_default();
    result.capabilities = maka_runtime::configuration::ModelCapabilities {
        chat: capabilities.chat.or(fallback.chat),
        vision: capabilities.vision.or(fallback.vision),
        reasoning: capabilities.reasoning.or(fallback.reasoning),
        function_calling: capabilities.function_calling.or(fallback.function_calling),
        parallel_tool_calls: capabilities
            .parallel_tool_calls
            .or(fallback.parallel_tool_calls),
        image_generation: capabilities.image_generation.or(fallback.image_generation),
        web_search: capabilities.web_search.or(fallback.web_search).or_else(|| {
            // Native provider defaults are independent of model generations.
            // Protocol-compatible endpoints must declare their own capability.
            match row.provider_type.as_str() {
                "openai" | "openai-codex" => Some(true),
                "anthropic" if known.is_some() => Some(true),
                _ => None,
            }
        }),
    };
    if let Some(threshold) = profile.and_then(|p| p.compaction_threshold) {
        result.compaction_threshold = Some(threshold);
    }
    result.supports_vision = capabilities
        .vision
        .or(fallback.vision)
        .unwrap_or(result.supports_vision);
    if let Some(declared) = profile.and_then(|p| p.thinking_levels.as_ref()) {
        use ThinkingLevel::*;
        let levels: Vec<_> = [Minimal, Low, Medium, High, Xhigh, Max]
            .into_iter()
            .filter(|level| declared.contains(level))
            .collect();
        if !levels.is_empty() {
            result.thinking_levels = levels;
        }
    }
    let no_text = model
        .modalities
        .as_ref()
        .or(metadata.modalities.as_ref())
        .is_some_and(|modalities| {
            !modalities.output.is_empty() && !modalities.output.contains(&ModelModality::Text)
        });
    let chat = capabilities.chat.or(fallback.chat);
    let unsupported = chat == Some(false)
        || (chat != Some(true) && no_text)
        || (capabilities.image_generation.or(fallback.image_generation) == Some(true)
            && chat != Some(true)
            && capabilities.reasoning.or(fallback.reasoning) != Some(true)
            && capabilities.function_calling.or(fallback.function_calling) != Some(true));
    result.can_use_as_chat_default = !provider.retired && !unsupported;
    result
}
