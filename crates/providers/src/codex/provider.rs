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

use super::{ADAPTER, Codex, login};
use futures_util::future::BoxFuture;
use maka_plugins::{
    http,
    model::{Credentials, ProviderKind},
    provider::{
        Connection, Context, Definition, Descriptor, Error, Model, Provider, Resolve,
        authentication::{Authenticate, Credential, Method},
    },
};
use maka_runtime::{
    configuration::{ApiProtocol, ModelCapabilities, ModelInfo},
    execution::ThinkingLevel,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

const ENDPOINT: &str = "https://chatgpt.com/backend-api/codex";
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    base_url: String,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct LoginInput {}

pub(super) fn configuration(connection: &Connection) -> Result<String, Error> {
    let configuration: Configuration = serde_json::from_value(connection.configuration.clone())
        .map_err(|_| Error::Invalid("invalid subscription configuration".into()))?;
    if configuration.base_url.trim_end_matches('/') != ENDPOINT {
        return Err(Error::Invalid(
            "subscription endpoint cannot be overridden".into(),
        ));
    }
    Ok(configuration.base_url)
}

impl Codex {
    pub fn definition(&self) -> Result<Definition, maka_plugins::Error> {
        Definition::new(
            Descriptor {
                label: "ChatGPT Subscription".into(),
                configuration_schema: serde_json::to_value(schemars::schema_for!(Configuration))
                    .map_err(|e| maka_plugins::Error::Invalid(e.to_string()))?,
                configuration_defaults: json!({ "baseUrl": ENDPOINT }),
                authentication: vec![Method {
                    id: "chatgpt".into(),
                    label: "Sign in with ChatGPT".into(),
                    input_schema: serde_json::to_value(schemars::schema_for!(LoginInput))
                        .map_err(|e| maka_plugins::Error::Invalid(e.to_string()))?,
                    interactive: true,
                }],
                discovery: true,
            },
            Arc::new(Self::default()),
        )
        .map_err(|e| maka_plugins::Error::Invalid(e.to_string()))
    }
}
impl Provider for Codex {
    fn resolve(&self, request: Resolve) -> BoxFuture<'_, Result<Model, Error>> {
        Box::pin(async move {
            let base_url = configuration(&request.connection)?;
            let info = match &request.overrides {
                Some(overrides) => overrides.apply(&request.model),
                None => request.model,
            };
            if info
                .api_protocol
                .is_some_and(|wire| wire != ApiProtocol::OpenaiResponses)
            {
                return Err(Error::Invalid(
                    "subscription requires the Responses protocol".into(),
                ));
            }
            let thinking_levels = request
                .overrides
                .as_ref()
                .and_then(|o| o.thinking_levels.clone())
                .unwrap_or_else(|| {
                    vec![
                        ThinkingLevel::Low,
                        ThinkingLevel::Medium,
                        ThinkingLevel::High,
                        ThinkingLevel::Xhigh,
                    ]
                });
            if request.thinking_level.is_some_and(|level| {
                level != ThinkingLevel::Off && !thinking_levels.contains(&level)
            }) {
                return Err(Error::Invalid(
                    "unsupported subscription thinking level".into(),
                ));
            }
            let parallel = info
                .capabilities
                .and_then(|c| c.parallel_tool_calls)
                .unwrap_or(true);
            let mut options = json!({"openai":{"store":false,"parallelToolCalls":parallel}});
            if let Some(level) = request
                .thinking_level
                .or(Some(ThinkingLevel::Medium).filter(|l| thinking_levels.contains(l)))
            {
                options["openai"]["reasoningEffort"] = match level {
                    ThinkingLevel::Off => json!("none"),
                    level => {
                        serde_json::to_value(level).map_err(|e| Error::Invalid(e.to_string()))?
                    }
                };
                if level != ThinkingLevel::Off {
                    options["openai"]["reasoningSummary"] = json!("auto");
                }
            }
            let model = Model {
                adapter: request
                    .overrides
                    .as_ref()
                    .and_then(|o| o.adapter.clone())
                    .unwrap_or_else(|| ADAPTER.into()),
                protocol: ProviderKind::OpenaiResponses,
                base_url,
                info,
                thinking_levels,
                provider_options: options,
            };
            model.validate()?;
            Ok(model)
        })
    }

    fn authorize(
        &self,
        connection: Connection,
        credential: Option<Credential>,
        session_id: String,
    ) -> BoxFuture<'_, Result<Credentials, Error>> {
        Box::pin(async move {
            configuration(&connection)?;
            let tokens = login::Tokens::read(&credential.ok_or(Error::AuthenticationRequired)?)?;
            let headers = super::request_headers(&tokens.access_token, &session_id)
                .map_err(|_| Error::Invalid("invalid subscription request identity".into()))?;
            Ok(Credentials::RequestHeaders(headers))
        })
    }

    fn authenticate(
        &self,
        request: Authenticate,
        context: Context,
    ) -> BoxFuture<'_, Result<Credential, Error>> {
        Box::pin(async move {
            serde_json::from_value::<LoginInput>(request.input.clone())
                .map_err(|_| Error::Invalid("invalid subscription login input".into()))?;
            login::authenticate(request, context).await
        })
    }

    fn refresh(
        &self,
        connection: Connection,
        credential: Credential,
        context: Context,
    ) -> BoxFuture<'_, Result<Credential, Error>> {
        Box::pin(async move {
            configuration(&connection)?;
            login::refresh(credential, context).await
        })
    }

    fn discover(
        &self,
        connection: Connection,
        credential: Option<Credential>,
        context: Context,
    ) -> BoxFuture<'_, Result<Vec<ModelInfo>, Error>> {
        Box::pin(async move {
            let base_url = configuration(&connection)?;
            let tokens = login::Tokens::read(&credential.ok_or(Error::AuthenticationRequired)?)?;
            let headers = super::request_headers(&tokens.access_token, &connection.id)
                .map_err(|_| Error::Invalid("invalid connection identity".into()))?;
            let response = context
                .transport
                .request(http::Request {
                    url: format!(
                        "{}/models?client_version=1.0.0",
                        base_url.trim_end_matches('/')
                    ),
                    method: http::Method::Get,
                    headers: headers.into_iter().collect(),
                    body: vec![],
                })
                .await
                .map_err(|_| Error::Transport("model discovery failed".into()))?;
            let result = async {
                if !(200..300).contains(&response.head.status) {
                    return Err(Error::AuthenticationRequired);
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .body
                    .next()
                    .await
                    .map_err(|_| Error::Transport("model inventory read failed".into()))?
                {
                    if chunk.len() > 4 * 1024 * 1024 - bytes.len() {
                        return Err(Error::Invalid("model inventory exceeds 4 MiB".into()));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                decode(&bytes)
            }
            .await;
            response.body.cancel();
            response
                .body
                .close()
                .await
                .map_err(|_| Error::Transport("inventory cleanup failed".into()))?;
            result
        })
    }
}

fn decode(bytes: &[u8]) -> Result<Vec<ModelInfo>, Error> {
    #[derive(Deserialize)]
    struct Inventory {
        models: Vec<Row>,
    }
    #[derive(Deserialize)]
    struct Row {
        slug: String,
        visibility: Option<String>,
        context_window: Option<u64>,
        #[serde(default = "priority")]
        priority: f64,
    }
    fn priority() -> f64 {
        10_000.0
    }
    let mut inventory: Inventory = serde_json::from_slice(bytes)
        .map_err(|_| Error::Invalid("invalid subscription inventory".into()))?;
    inventory
        .models
        .sort_by(|a, b| a.priority.total_cmp(&b.priority));
    inventory
        .models
        .into_iter()
        .filter(|r| {
            !r.slug.trim().is_empty()
                && !r
                    .visibility
                    .as_ref()
                    .is_some_and(|v| matches!(v.trim().to_lowercase().as_str(), "hide" | "hidden"))
        })
        .map(|row| {
            let mut info = ModelInfo::new(row.slug);
            info.context_window = row.context_window.filter(|n| *n > 0);
            info.capabilities = Some(ModelCapabilities {
                chat: Some(true),
                vision: Some(true),
                reasoning: Some(true),
                function_calling: Some(true),
                parallel_tool_calls: Some(true),
                web_search: Some(true),
                ..Default::default()
            });
            info.validate().map_err(Error::Invalid)?;
            Ok(info)
        })
        .collect()
}
