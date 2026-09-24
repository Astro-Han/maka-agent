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

use super::wire_limit;
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
    pub default_thinking_level: Option<ThinkingLevel>,
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
        if self
            .default_thinking_level
            .is_some_and(|level| !self.thinking_levels.contains(&level))
        {
            return Err(ConfigError::Invalid(
                "unsupported default thinking level".into(),
            ));
        }
        Ok(())
    }
}

pub(super) fn resolve(
    row: &ConnectionCatalogEntry,
    reported: &ModelInfo,
    default: Option<&str>,
) -> ModelCatalogEntry {
    let id = reported.id.trim();
    let profile = row
        .model_overrides
        .as_ref()
        .and_then(|profiles| profiles.get(id));
    let effective = profile.map(|profile| profile.apply(reported));
    let model = effective.as_ref().unwrap_or(reported);
    let capabilities = model.capabilities.unwrap_or_default();
    let no_text = model.modalities.as_ref().is_some_and(|modalities| {
        !modalities.output.is_empty() && !modalities.output.contains(&ModelModality::Text)
    });
    let chat = capabilities.chat;
    let unsupported = chat == Some(false)
        || (chat != Some(true) && no_text)
        || (capabilities.image_generation == Some(true)
            && chat != Some(true)
            && capabilities.reasoning != Some(true)
            && capabilities.function_calling != Some(true));
    let thinking_levels = profile
        .and_then(|p| p.thinking_levels.clone())
        .or_else(|| model.thinking_levels.clone())
        .unwrap_or_default();
    ModelCatalogEntry {
        id: id.into(),
        capabilities,
        can_use_as_chat_default: !unsupported,
        is_default: default == Some(id),
        supports_vision: capabilities.vision.unwrap_or(false),
        default_supports_vision: reported.capabilities.and_then(|c| c.vision),
        default_context_window: reported.context_window,
        default_input_limit: reported.input_limit,
        default_thinking_level: profile
            .and_then(|p| p.default_thinking_level)
            .or(model.default_thinking_level)
            .filter(|level| thinking_levels.contains(level)),
        thinking_levels,
        input_limit: model.input_limit,
        compaction_threshold: profile.and_then(|p| p.compaction_threshold),
        display_name: model.display_name.as_deref().map(|s| wire_limit(s, 512)),
        description: model.description.as_deref().map(|s| wire_limit(s, 2048)),
        context_window: model.context_window,
        knowledge_cutoff: model.knowledge_cutoff.clone(),
    }
}
