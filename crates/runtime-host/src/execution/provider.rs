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

use crate::provider_route::{self, Wire};
use crate::session::SessionConfiguration;
use maka_config::{ConfigurationStore, model_catalog};
use maka_model::ProviderConfig;
use maka_protocol::{OperationError, OperationErrorCode};
use maka_runtime::configuration::*;
use maka_runtime::context::ModelRequestContext;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

mod auth;
mod context;
mod options;
mod output;

pub(super) struct PreparedProvider {
    pub tool_mode: maka_runtime::execution::ToolMode,
    pub editing_tools: maka_runtime::execution::EditingTools,
    pub config: ProviderConfig,
    pub options: Value,
    pub supports_vision: bool,
    pub context: ModelRequestContext,
    pub main_output_limit: Option<u64>,
    binding: Option<Arc<auth::Binding>>,
}

impl PreparedProvider {
    pub(super) fn admit(self, oauth: &crate::oauth::Authority) -> Result<Self, OperationError> {
        if let Some(binding) = &self.binding {
            binding.admit(oauth)?;
        }
        Ok(self)
    }
}

fn unavailable(message: impl Into<String>) -> OperationError {
    OperationError {
        code: OperationErrorCode::OperationUnavailable,
        message: message.into(),
    }
}

pub(super) async fn resolve(
    config: &Arc<ConfigurationStore>,
    oauth: &crate::oauth::Authority,
    session_id: &str,
    session: &SessionConfiguration,
) -> Result<PreparedProvider, OperationError> {
    observe(config, session_id, session).await?.admit(oauth)
}

/// Read-only configuration and credential identity, without execution authority.
pub(super) async fn observe(
    config: &Arc<ConfigurationStore>,
    session_id: &str,
    session: &SessionConfiguration,
) -> Result<PreparedProvider, OperationError> {
    let model = session
        .target
        .model()
        .ok_or_else(|| unavailable("Executor Session has no model backend"))?;
    observe_binding(config, session_id, model, session.thinking_level).await
}

/// The caller supplies the admitted model identity, never a replacement Session.
pub(super) async fn observe_binding(
    config: &Arc<ConfigurationStore>,
    session_id: &str,
    target: &maka_runtime::execution::ModelBinding,
    thinking_level: Option<maka_runtime::execution::ThinkingLevel>,
) -> Result<PreparedProvider, OperationError> {
    let network = config
        .network_configuration()
        .await
        .map_err(crate::server::configuration::failure)?;
    let network = maka_network::Policy::from_settings(&network.proxy, network.password.as_deref())
        .map_err(|error| unavailable(error.to_string()))?;
    let catalog = config.catalog().await.map_err(|error| OperationError {
        code: if matches!(error, maka_config::ConfigError::CommitUnknown) {
            OperationErrorCode::CommitOutcomeUnknown
        } else {
            OperationErrorCode::PersistenceFailed
        },
        message: error.to_string().chars().take(1024).collect(),
    })?;
    let row = catalog
        .connections
        .iter()
        .find(|row| {
            row.connection_id == target.connection_id
                && row.slug == target.connection_slug
                && row.enabled
                && row.enabled_model_ids.contains(&target.model)
        })
        .ok_or_else(|| {
            unavailable("Session model connection or enabled model is no longer available")
        })?;
    let facts = model_catalog::provider_facts(&row.provider_type)
        .map_err(|_| unavailable("Provider facts are unavailable"))?;
    if facts.retired || facts.broken_model_ids.contains(&target.model) {
        return Err(unavailable("Provider or model is retired or unavailable"));
    }
    let models = model_catalog::resolve(row, None)
        .map_err(|_| unavailable("Cannot resolve model capabilities"))?;
    let model = models
        .iter()
        .find(|model| model.id == target.model && model.can_use_as_chat_default)
        .ok_or_else(|| unavailable("Session model is not available for chat"))?;
    if let Some(level) = thinking_level
        && !model.thinking_levels.contains(&level)
    {
        return Err(unavailable("Session thinking level is no longer supported"));
    }
    let route = provider_route::resolve(row, facts, &target.model)?;
    let overrides = row
        .model_overrides
        .as_ref()
        .and_then(|models| models.get(&target.model));
    let tool_mode = maka_runtime::execution::ToolMode::for_model(
        &target.model,
        &route.base_url,
        overrides.and_then(|value| value.code_mode),
    );
    let editing_tools = maka_runtime::execution::EditingTools::for_model(
        &target.model,
        &route.base_url,
        overrides.and_then(|value| value.apply_patch),
    );
    route.check_execution()?;
    let options = options::resolve(row, facts, &target.model, thinking_level, &route)?;
    let main_output_limit = output::resolve(row, facts, &target.model, route.wire, &options)?;
    let endpoint = row.base_url.as_deref().unwrap_or(&facts.base_url);
    let expected = ConnectionCredentialTarget {
        connection_id: row.connection_id.clone(),
        revision: row.revision,
        slug: row.slug.clone(),
        provider_type: row.provider_type.clone(),
        effective_base_url: endpoint.to_owned(),
    };
    let secret = async |kind| {
        config
            .credential_secret(
                &CredentialLocator::Connection {
                    connection_id: row.connection_id.clone(),
                    kind,
                },
                Some(&expected),
            )
            .await
            .map_err(|_| unavailable("Credential connection basis changed or vault is unavailable"))
    };
    let mut binding = None;
    let auth = match facts.auth_kind {
        ProviderAuthKind::ApiKey => maka_model::ProviderAuth::ApiKey(
            secret(ConnectionCredentialKind::ApiKey)
                .await?
                .filter(|key| !key.trim().is_empty())
                .ok_or_else(|| unavailable("Provider API key is not configured"))?,
        ),
        ProviderAuthKind::OauthToken => {
            let observed = auth::observe(config, expected.clone(), session_id).await?;
            let auth = observed.auth()?;
            binding = Some(observed);
            auth
        }
        ProviderAuthKind::None | ProviderAuthKind::OptionalApiKey => {
            return Err(unavailable(
                "Provider authentication profile is not installed",
            ));
        }
    };
    let headers = secret(ConnectionCredentialKind::RequestHeaders)
        .await?
        .map(|value| serde_json::from_str::<BTreeMap<String, String>>(&value))
        .transpose()
        .map_err(|_| unavailable("Invalid provider request headers"))?
        .unwrap_or_default();
    let config = ProviderConfig {
        adapter: overrides
            .and_then(|value| value.adapter.clone())
            .or_else(|| {
                (row.provider_type == "openai-codex").then(|| maka_providers::codex::ADAPTER.into())
            }),
        capabilities: model.capabilities,
        kind: route.kind,
        model: target.model.clone(),
        base_url: route.base_url,
        auth,
        headers,
        network,
        body_overlay: row.request_body_overlay.as_ref().map(|value| {
            value
                .as_object()
                .expect("validated request body overlay")
                .clone()
        }),
    };
    Ok(PreparedProvider {
        tool_mode,
        editing_tools,
        binding,
        config,
        options,
        supports_vision: model.supports_vision,
        context: context::resolve(row, facts, &target.model)?,
        main_output_limit,
    })
}
