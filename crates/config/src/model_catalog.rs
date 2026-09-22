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

use crate::{ConfigError, Result};
pub mod adapter;
pub mod discovery;
use adapter::{AdapterKind, ModelRuntimeOverride, RuntimeAdapter};
use discovery::{DiscoveryAuth, ModelDiscovery};
mod entry;
mod limits;
mod metadata;
pub use entry::ModelCatalogEntry;
pub(crate) use limits::validate_overrides;
pub use limits::{ModelLimits, resolve_limits};
use maka_runtime::configuration::{
    ApiProtocol, ConnectionCatalogEntry, ModelDiscoverySource, ModelInfo, ProviderAuthKind,
};
pub use metadata::{ModelMetadata, ThinkingOffBehavior};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    sync::OnceLock,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFacts {
    pub label: String,
    pub base_url: String,
    pub auth_kind: ProviderAuthKind,
    pub runtime_adapter: RuntimeAdapter,
    pub protocol_adapters: BTreeMap<ApiProtocol, RuntimeAdapter>,
    pub retired: bool,
    pub supports_model_discovery: bool,
    pub model_discovery: ModelDiscovery,
    pub fallback_models: Vec<String>,
    pub models: BTreeMap<String, ModelFacts>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFacts {
    pub metadata: ModelMetadata,
    pub runtime_override: Option<ModelRuntimeOverride>,
    pub entry: ModelCatalogEntry,
    /// Native Anthropic SDK capability, generated with the bundled SDK version.
    pub anthropic_adaptive_thinking: Option<bool>,
}

impl ProviderFacts {
    /// Select by the registry's wire contract, not by the provider's marketing identity.
    /// Filters, alternate envelopes, paths and auth need their own implemented semantics.
    pub fn native_model_list(&self) -> Option<maka_runtime::configuration::ModelListProtocol> {
        use maka_runtime::configuration::ModelListProtocol;
        if self.retired {
            return None;
        }
        let ModelDiscovery::Protocol {
            auth,
            path: None,
            query: None,
            response_shape: None,
            model_protocols: None,
            filter: None,
        } = &self.model_discovery
        else {
            return None;
        };
        if self.auth_kind == ProviderAuthKind::OauthToken {
            return match auth {
                Some(DiscoveryAuth::OpenaiCodex) => Some(ModelListProtocol::Codex),
                Some(DiscoveryAuth::GithubCopilot) => Some(ModelListProtocol::Copilot),
                Some(DiscoveryAuth::OauthBearer)
                    if matches!(
                        self.runtime_adapter.kind,
                        AdapterKind::Openai { .. } | AdapterKind::OpenaiCompatible { .. }
                    ) =>
                {
                    Some(ModelListProtocol::Openai)
                }
                _ => None,
            };
        }
        if self.auth_kind != ProviderAuthKind::ApiKey || auth.is_some() {
            return None;
        }
        match self.runtime_adapter.kind {
            AdapterKind::Openai { .. } | AdapterKind::OpenaiCompatible { .. } => {
                Some(ModelListProtocol::Openai)
            }
            AdapterKind::Anthropic { .. } => Some(ModelListProtocol::Anthropic),
            _ => None,
        }
    }
}

fn facts() -> Result<&'static BTreeMap<String, ProviderFacts>> {
    static FACTS: OnceLock<std::result::Result<BTreeMap<String, ProviderFacts>, String>> =
        OnceLock::new();
    FACTS
        .get_or_init(|| {
            serde_json::from_str(include_str!(concat!(
                env!("OUT_DIR"),
                "/catalog-facts.json"
            )))
            .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| ConfigError::Invalid(error.clone()))
}

pub fn provider_facts(provider: &str) -> Result<&'static ProviderFacts> {
    facts()?
        .get(provider)
        .ok_or_else(|| ConfigError::Invalid(format!("unknown provider: {provider}")))
}

/// Resolve dynamic connection selections against source-derived immutable build facts.
pub fn resolve(
    row: &ConnectionCatalogEntry,
    default: Option<&str>,
) -> Result<Vec<ModelCatalogEntry>> {
    let Some(provider) = facts()?.get(&row.provider_type) else {
        return Ok(vec![]);
    };
    let default = default.map(str::trim).filter(|id| !id.is_empty());
    let mut models = row.models.clone();
    if !provider.supports_model_discovery {
        let mut baseline: Vec<ModelInfo> = provider
            .fallback_models
            .iter()
            .map(|id| {
                models
                    .iter()
                    .rev()
                    .find(|model| model.id == *id)
                    .cloned()
                    .unwrap_or_else(|| fallback_model(provider, id))
            })
            .collect();
        baseline.extend(
            models
                .into_iter()
                .filter(|model| !provider.fallback_models.contains(&model.id)),
        );
        models = baseline;
    } else if models.is_empty() && row.model_source != Some(ModelDiscoverySource::Fetched) {
        models = provider
            .fallback_models
            .iter()
            .map(|id| fallback_model(provider, id))
            .collect();
    }
    let mut seen = HashSet::new();
    models.retain(|model| {
        let id = model.id.trim();
        !id.is_empty() && seen.insert(id.to_owned())
    });
    if let Some(id) = default
        && seen.insert(id.to_owned())
    {
        models.insert(0, ModelInfo::new(id));
    }
    for id in row
        .enabled_model_ids
        .iter()
        .chain(row.model_overrides.iter().flat_map(|values| values.keys()))
    {
        let id = id.trim();
        if !id.is_empty() && seen.insert(id.to_owned()) {
            models.push(ModelInfo::new(id));
        }
    }
    Ok(models
        .iter()
        .map(|model| entry::resolve(row, provider, model, default))
        .collect())
}

fn fallback_model(provider: &ProviderFacts, id: &str) -> ModelInfo {
    let mut model = ModelInfo::new(id);
    if let Some(name) = provider
        .models
        .get(id)
        .and_then(|facts| facts.metadata.display_name.as_deref())
        && !name.is_empty()
    {
        model.display_name = Some(wire_limit(name, 512));
    }
    model
}

pub(crate) fn wire_limit(text: &str, max: usize) -> String {
    let mut units = 0;
    text.chars()
        .take_while(|character| {
            units += character.len_utf16();
            units <= max
        })
        .collect()
}

// Exact family/boundary semantics of core's first-party Claude fallback.
fn claude_vision(provider: &str, id: &str) -> bool {
    if !matches!(provider, "anthropic" | "claude-subscription") {
        return false;
    }
    let Some(mut tail) = id.strip_prefix("claude-") else {
        return false;
    };
    loop {
        for family in ["opus", "sonnet", "haiku", "fable"] {
            if let Some(rest) = tail.strip_prefix(family)
                && rest
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
            {
                return true;
            }
        }
        let Some((part, rest)) = tail.split_once('-') else {
            return false;
        };
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return false;
        }
        tail = rest;
    }
}
