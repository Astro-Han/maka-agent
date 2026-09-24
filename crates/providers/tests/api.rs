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

use maka_plugins::{
    composition::Scope,
    contributions::Catalog,
    fiber::Fiber,
    provider::{Binding, Connection, Identity, Resolve},
};
use maka_providers::api::ApiProviders;
use maka_runtime::{
    configuration::{ApiProtocol, ModelInfo, ModelOverride},
    execution::ThinkingLevel::{High, Low, Max, Medium, Off, Xhigh},
    model::request::ProviderKind,
};
use serde_json::json;

#[path = "api/discovery.rs"]
mod discovery;
#[path = "api/effects.rs"]
mod effects;

fn registry() -> (Catalog, Fiber, maka_plugins::Registration) {
    // Bundled code receives no additional authority from its distribution identity.
    let owner = Fiber::new("external.copy", "external.entry", Scope::Profile).unwrap();
    owner.begin_loading().unwrap();
    owner.ready().unwrap();
    owner.publish().unwrap();
    let catalog = Catalog::default();
    let registration = catalog
        .register(&owner.context(), ApiProviders.stage().unwrap())
        .unwrap();
    (catalog, owner, registration)
}
fn binding(catalog: &Catalog, name: &str) -> Binding {
    Binding::resolve(
        &Identity {
            package_id: "external.copy".into(),
            entry_id: "external.entry".into(),
            scope: Scope::Profile,
            name: name.into(),
        },
        catalog,
    )
    .unwrap()
}
fn request(binding: &Binding, id: &str, overrides: ModelOverride) -> Resolve {
    let configuration = binding
        .definition()
        .configure(json!({"baseUrl":"https://relay.test/v1"}))
        .unwrap();
    Resolve {
        connection: Connection {
            id: "test-account".into(),
            revision: 1,
            configuration,
        },
        model: ModelInfo::new(id),
        overrides: Some(overrides),
        thinking_level: None,
    }
}

#[tokio::test]
async fn public_provider_binding_preserves_protocol_contracts_and_explicit_policy() {
    let (catalog, _owner, _registration) = registry();
    for name in ["ollama", "localai"] {
        let binding = binding(&catalog, name);
        assert!(binding.definition().descriptor().anonymous);
        let connection = request(&binding, "custom", ModelOverride::default()).connection;
        let credentials = binding
            .authorize(connection, None, "session".into())
            .await
            .unwrap();
        assert!(
            matches!(credentials, maka_plugins::model::Credentials::RequestHeaders(headers) if headers.is_empty())
        );
    }
    let required = binding(&catalog, "openai");
    assert!(!required.definition().descriptor().anonymous);
    assert!(
        required
            .authorize(
                request(&required, "custom", ModelOverride::default()).connection,
                None,
                "session".into()
            )
            .await
            .is_err()
    );
    for (name, summary) in [
        ("deepseek", false),
        ("moonshot-global", true),
        ("alibaba-token-plan", true),
    ] {
        let binding = binding(&catalog, name);
        let input = request(
            &binding,
            "custom",
            ModelOverride {
                api_protocol: Some(ApiProtocol::OpenaiResponses),
                thinking_levels: Some(vec![Off, High]),
                default_thinking_level: Some(High),
                ..Default::default()
            },
        );
        let model = binding.prepare(input.clone()).await.unwrap();
        assert!(matches!(model.protocol, ProviderKind::OpenResponses(_)));
        assert_eq!(model.adapter, "responses");
        assert_eq!(
            model.provider_options["openResponses"]["reasoningEffort"],
            "high"
        );
        assert_eq!(
            model.provider_options["openResponses"]
                .get("reasoningSummary")
                .is_some(),
            summary
        );
        let model = binding
            .prepare(Resolve {
                thinking_level: Some(Off),
                ..input
            })
            .await
            .unwrap();
        assert_eq!(
            model.provider_options,
            json!({"openResponses":{"reasoningEffort":"none"}})
        );
    }
    for (name, protocol, adapter) in [
        (
            "openai-compatible",
            ApiProtocol::OpenaiChat,
            "chat-completions",
        ),
        (
            "openai-responses-compatible",
            ApiProtocol::OpenaiResponses,
            "responses",
        ),
        (
            "anthropic-compatible",
            ApiProtocol::AnthropicMessages,
            "anthropic-messages",
        ),
    ] {
        let binding = binding(&catalog, name);
        let mut input = request(
            &binding,
            "future-model",
            ModelOverride {
                api_protocol: Some(protocol),
                max_output_tokens: Some(5000),
                ..Default::default()
            },
        );
        input.model.max_output_tokens = Some(4000);
        let model = binding.prepare(input).await.unwrap();
        assert_eq!(model.adapter, adapter);
        assert_eq!(model.main_output_limit, Some(4000));
        assert_eq!(model.info.max_output_tokens, Some(4000));
    }
}

#[tokio::test]
async fn account_metadata_and_reply_capacity_survive_plugin_policy_resolution() {
    let (catalog, _owner, _registration) = registry();
    let anthropic = binding(&catalog, "anthropic");
    let mut input = request(
        &anthropic,
        "claude-sonnet-4-5",
        ModelOverride {
            max_output_tokens: Some(5000),
            ..Default::default()
        },
    );
    input.model.max_output_tokens = Some(4000);
    let model = anthropic.prepare(input.clone()).await.unwrap();
    assert_eq!(
        model.provider_options["anthropic"]["thinking"]["budgetTokens"],
        1024
    );
    assert_eq!(model.main_output_limit, Some(2976));
    input.overrides.as_mut().unwrap().max_output_tokens = Some(1000);
    assert!(anthropic.prepare(input).await.is_err());
    let openai = binding(&catalog, "openai");
    for (id, label) in [("gpt-6-sol", "GPT-6 Sol"), ("gpt-6-luna", "GPT-6 Luna")] {
        let mut input = request(&openai, id, ModelOverride::default());
        input.thinking_level = Some(Max);
        let model = openai.prepare(input).await.unwrap();
        assert!(matches!(model.protocol, ProviderKind::OpenaiResponses));
        assert_eq!(model.info.id, id);
        assert_eq!(model.info.display_name.as_deref(), Some(label));
        assert_eq!(model.thinking_levels, [Low, Medium, High, Xhigh, Max]);
        assert_eq!(model.provider_options["openai"]["reasoningEffort"], "max");
    }
    let input = request(&openai, "gpt-5.2", ModelOverride::default());
    let model = openai.prepare(input.clone()).await.unwrap();
    assert_eq!(
        model.provider_options["openai"]["reasoningEffort"],
        "medium"
    );
    assert_eq!(model.provider_options["openai"]["reasoningSummary"], "auto");
    let mut account = input;
    account.model.thinking_levels = Some(vec![High]);
    let model = openai.prepare(account).await.unwrap();
    assert!(
        model.provider_options["openai"]
            .get("reasoningEffort")
            .is_none()
    );
    let mut input = request(
        &openai,
        "gpt-5.2",
        ModelOverride {
            thinking_levels: Some(vec![Max]),
            default_thinking_level: Some(Max),
            ..Default::default()
        },
    );
    input.model.context_window = Some(12345);
    input.model.supports_reasoning_summary = Some(false);
    input.model.thinking_levels = Some(vec![]);
    let model = openai.prepare(input).await.unwrap();
    assert_eq!(model.info.context_window, Some(12345));
    assert_eq!(model.thinking_levels, [Max]);
    assert_eq!(model.provider_options["openai"]["reasoningEffort"], "max");
    assert!(
        model.provider_options["openai"]
            .get("reasoningSummary")
            .is_none()
    );
    assert_eq!(model.main_output_limit, None);
}
