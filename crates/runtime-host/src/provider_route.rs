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

use maka_config::model_catalog::{
    ProviderFacts,
    adapter::{
        AdapterKind, AdapterName, AnthropicAuth, OpenaiReasoningReplay, ResponsesContract,
        RuntimeAdapter,
    },
};
use maka_model::ProviderKind;
use maka_protocol::{OperationError, OperationErrorCode};
use maka_runtime::configuration::ConnectionCatalogEntry;

pub(crate) use maka_runtime::configuration::ApiProtocol as Wire;

fn supports(adapter: &RuntimeAdapter, wire: Wire) -> bool {
    match &adapter.kind {
        AdapterKind::OpenaiCodex { .. } => wire == Wire::OpenaiResponses,
        AdapterKind::Openai { api_protocol, .. } => {
            wire != Wire::AnthropicMessages && api_protocol.is_none_or(|selected| selected == wire)
        }
        AdapterKind::OpenaiCompatible { responses, .. } => {
            wire == Wire::OpenaiChat || (wire == Wire::OpenaiResponses && responses.is_some())
        }
        AdapterKind::Anthropic { .. } => wire == Wire::AnthropicMessages,
        AdapterKind::Google { .. } | AdapterKind::Cohere | AdapterKind::Unavailable => false,
    }
}

fn check_execution(adapter: &RuntimeAdapter, wire: Wire) -> Result<(), OperationError> {
    let supported = match &adapter.kind {
        AdapterKind::Openai { responses, .. } | AdapterKind::OpenaiCodex { responses } => {
            wire != Wire::OpenaiResponses
                || !matches!(
                    responses,
                    ResponsesContract::Openai {
                        reasoning_replay: OpenaiReasoningReplay::None
                    }
                )
        }
        AdapterKind::Anthropic {
            auth: AnthropicAuth::ApiKey,
            include_beta_headers: None,
            ..
        } => true,
        AdapterKind::OpenaiCompatible {
            responses: Some(_), ..
        } if wire == Wire::OpenaiResponses => true,
        AdapterKind::OpenaiCompatible {
            include_usage: None,
            replay_assistant_reasoning_as: None,
            replay_assistant_reasoning_details: None,
            normalize_usage: None,
            ..
        } => wire == Wire::OpenaiChat,
        _ => false,
    };
    if !supported {
        return Err(unavailable(
            "Provider execution profile or semantic features are not supported",
        ));
    }
    Ok(())
}

pub(crate) struct Route<'a> {
    pub wire: Wire,
    pub kind: ProviderKind,
    pub base_url: String,
    adapter: &'a RuntimeAdapter,
}

impl Route<'_> {
    /// A one-shot HTTP probe has no streaming or reasoning-replay requirements.
    pub(crate) fn check_probe(&self) -> Result<(), OperationError> {
        if matches!(
            self.adapter.kind,
            AdapterKind::Anthropic {
                auth: AnthropicAuth::Bearer,
                ..
            }
        ) {
            return Err(unavailable(
                "Provider probe authentication is not supported",
            ));
        }
        Ok(())
    }

    pub(crate) fn check_execution(&self) -> Result<(), OperationError> {
        check_execution(self.adapter, self.wire)
    }
}

fn unavailable(message: impl Into<String>) -> OperationError {
    OperationError {
        code: OperationErrorCode::OperationUnavailable,
        message: message.into(),
    }
}

fn adapter(value: &RuntimeAdapter) -> Result<&RuntimeAdapter, OperationError> {
    match value.kind {
        AdapterKind::Google { .. } | AdapterKind::Cohere | AdapterKind::Unavailable => {
            Err(unavailable("Provider adapter or protocol is not supported"))
        }
        _ => Ok(value),
    }
}

mod preferred;
use preferred::resolve as preferred;

pub(crate) fn resolve<'a>(
    row: &ConnectionCatalogEntry,
    facts: &'a ProviderFacts,
    model: &str,
) -> Result<Route<'a>, OperationError> {
    let known = facts.models.get(model);
    let override_ = known.and_then(|model| model.runtime_override.as_ref());
    let base = adapter(
        override_
            .map(|value| &value.adapter)
            .unwrap_or(&facts.runtime_adapter),
    )?;
    let stored = row.models.iter().find(|entry| entry.id == model);
    let explicit = row
        .model_overrides
        .as_ref()
        .and_then(|values| values.get(model))
        .and_then(|value| value.api_protocol);
    let declared = explicit.or_else(|| stored.and_then(|model| model.api_protocol));
    let wire = match declared {
        Some(wire) => wire,
        None => {
            let preferred = preferred(&row.provider_type, model);
            if supports(base, preferred) {
                preferred
            } else if supports(base, Wire::OpenaiChat) {
                Wire::OpenaiChat
            } else if supports(base, Wire::OpenaiResponses) {
                Wire::OpenaiResponses
            } else {
                Wire::AnthropicMessages
            }
        }
    };
    let uses_override = supports(base, wire);
    let selected = if uses_override {
        base
    } else {
        let alternate = facts
            .protocol_adapters
            .get(&wire)
            .map(adapter)
            .transpose()?;
        match alternate {
            Some(value) if supports(value, wire) => value,
            _ => adapter(&facts.runtime_adapter)?,
        }
    };
    if !supports(selected, wire) {
        return Err(unavailable(
            "Provider does not support the declared model protocol",
        ));
    }
    let resolved_endpoint = row
        .base_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .or_else(|| {
            uses_override
                .then(|| override_.and_then(|value| value.base_url.as_deref()))
                .flatten()
        })
        .unwrap_or(&facts.base_url);
    let mut url =
        url::Url::parse(resolved_endpoint).map_err(|_| unavailable("Invalid provider endpoint"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(unavailable("Invalid provider endpoint"));
    }
    let path = url.path().trim_end_matches('/');
    let normalize_base_url = match selected.kind {
        AdapterKind::Anthropic {
            normalize_base_url, ..
        } => normalize_base_url,
        AdapterKind::OpenaiCompatible {
            normalize_base_url, ..
        } => normalize_base_url.unwrap_or(false),
        _ => false,
    };
    let path = if normalize_base_url {
        let root = if path.to_ascii_lowercase().ends_with("/v1") {
            &path[..path.len() - 3]
        } else {
            path
        };
        format!("{root}/v1")
    } else if wire == Wire::OpenaiResponses && path.to_ascii_lowercase().ends_with("/responses") {
        path[..path.len() - 10].to_owned()
    } else {
        path.to_owned()
    };
    url.set_path(&path);
    let kind = match wire {
        Wire::OpenaiChat
            if row.provider_type == "openai-compatible"
                && matches!(selected.kind, AdapterKind::OpenaiCompatible { .. }) =>
        {
            ProviderKind::OpenaiCompatible {
                name: if matches!(
                    selected.kind,
                    AdapterKind::OpenaiCompatible {
                        name: AdapterName::Connection,
                        ..
                    }
                ) {
                    row.slug.clone()
                } else {
                    row.provider_type.clone()
                },
            }
        }
        Wire::OpenaiChat => ProviderKind::OpenaiChat,
        Wire::OpenaiResponses => {
            let responses = match &selected.kind {
                AdapterKind::Openai { responses, .. } | AdapterKind::OpenaiCodex { responses } => {
                    responses
                }
                AdapterKind::OpenaiCompatible {
                    responses: Some(responses),
                    ..
                } => responses,
                _ => return Err(unavailable("Responses contract is not declared")),
            };
            match responses {
                ResponsesContract::Openai { .. } => ProviderKind::OpenaiResponses,
                ResponsesContract::OpenResponses { contract } => {
                    ProviderKind::OpenResponses(*contract)
                }
            }
        }
        Wire::AnthropicMessages => ProviderKind::Anthropic,
    };
    Ok(Route {
        wire,
        kind,
        base_url: url.to_string(),
        adapter: selected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_config::model_catalog::provider_facts;
    use serde_json::json;

    #[test]
    fn custom_chat_and_responses_use_distinct_adapters_without_changing_public_wire() {
        for provider in ["openai-compatible", "openai-responses-compatible"] {
            let row: ConnectionCatalogEntry = serde_json::from_value(json!({
                "connectionId":"relay", "revision":1, "slug":"custom-chat-relay",
                "name":"Relay", "providerType":provider, "enabled":true,
                "baseUrl":"https://relay.example/v1", "enabledModelIds":["unknown"],
                "models":[]
            }))
            .unwrap();
            let route = resolve(&row, provider_facts(provider).unwrap(), "unknown").unwrap();
            route.check_execution().unwrap();
            match route.kind {
                ProviderKind::OpenaiCompatible { name } => {
                    assert_eq!(provider, "openai-compatible");
                    assert_eq!(name, "custom-chat-relay");
                    assert!(route.wire == Wire::OpenaiChat);
                }
                ProviderKind::OpenaiResponses => {
                    assert_eq!(provider, "openai-responses-compatible");
                    assert!(route.wire == Wire::OpenaiResponses);
                }
                kind => panic!("unexpected relay adapter: {kind:?}"),
            }
        }
    }

    #[test]
    fn protocol_declaration_changes_only_a_supported_wire_without_rewriting_inventory() {
        let mut row: ConnectionCatalogEntry = serde_json::from_value(json!({
            "connectionId":"connection","revision":1,"slug":"fixture","name":"Fixture",
            "providerType":"openai","enabled":true,"enabledModelIds":["gpt-5"],
            "models":[{"id":"gpt-5","apiProtocol":"openai-responses"}]
        }))
        .unwrap();
        let facts = provider_facts("openai").unwrap();
        assert_eq!(
            resolve(&row, facts, "gpt-5").unwrap().wire,
            Wire::OpenaiResponses
        );
        for (protocol, expected) in [
            ("openai-chat", Some(Wire::OpenaiChat)),
            ("anthropic-messages", None),
        ] {
            row.model_overrides = Some(
                serde_json::from_value(json!({
                    "gpt-5":{"apiProtocol":protocol}
                }))
                .unwrap(),
            );
            assert_eq!(
                resolve(&row, facts, "gpt-5").ok().map(|route| route.wire),
                expected
            );
            assert_eq!(row.models[0].api_protocol, Some(Wire::OpenaiResponses));
        }
    }
}
