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

//! Bundled API providers use the same registration and secret contracts as plugins.
use crate::facts::{self, AuthKind, ProviderFacts};
use futures_util::future::BoxFuture;
use maka_plugins::{
    contributions::Staged,
    kernel::{Plugin, PluginContext},
    provider::{Connection, Definition, Descriptor, Error, Model, Provider, Resolve},
};
use maka_runtime::configuration::{ApiProtocol as Wire, ModelInfo, ModelOverride};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
mod authentication;
mod discovery;
mod effects;
mod inventory;
mod metadata;
mod options;
mod output;
mod preferred;
mod route;

pub const ID: &str = "maka.providers";

pub struct ApiProviders;
impl ApiProviders {
    pub fn stage(&self) -> Result<Staged, String> {
        let mut staged = Staged::default();
        for (id, facts) in facts::all().map_err(|error| error.to_string())? {
            if facts.retired || facts.auth_kind == AuthKind::OauthToken {
                continue;
            }
            staged
                .insert(id.clone(), ApiProvider { id, facts }.definition()?)
                .map_err(|error| error.to_string())?;
        }
        Ok(staged)
    }
}
impl Plugin for ApiProviders {
    fn activate(&self, _: PluginContext, _: Value) -> BoxFuture<'static, Result<Staged, String>> {
        let result = self.stage();
        Box::pin(async { result })
    }
}

#[derive(Clone)]
struct ApiProvider {
    id: &'static str,
    facts: &'static ProviderFacts,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Configuration {
    #[schemars(length(min = 1, max = 2048))]
    base_url: String,
}
impl Configuration {
    fn read(connection: &Connection) -> Result<Self, Error> {
        let mut configuration: Self = serde_json::from_value(connection.configuration.clone())
            .map_err(|_| Error::Invalid("invalid API provider configuration".into()))?;
        configuration.base_url =
            maka_runtime::configuration::validation::normalize_base_url(&configuration.base_url)
                .map_err(Error::Invalid)?;
        Ok(configuration)
    }
}

/// Pure policy input, not a Host catalog or a credential handle.
struct Selection<'a> {
    provider: &'a str,
    connection: &'a Connection,
    base_url: &'a str,
    model: &'a ModelInfo,
    overrides: Option<&'a ModelOverride>,
}

fn unavailable(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

impl ApiProvider {
    fn definition(&self) -> Result<Definition, String> {
        let authentication =
            authentication::methods(self.facts.auth_kind).map_err(|error| error.to_string())?;
        Definition::new(
            Descriptor {
                label: self.facts.label.clone(),
                configuration_schema: serde_json::to_value(schemars::schema_for!(Configuration))
                    .map_err(|error| error.to_string())?,
                configuration_defaults: if self.facts.base_url.is_empty() {
                    json!({})
                } else {
                    json!({"baseUrl":self.facts.base_url})
                },
                authentication,
                anonymous: matches!(
                    self.facts.auth_kind,
                    AuthKind::None | AuthKind::OptionalApiKey
                ),
                discovery: true,
            },
            Arc::new(self.clone()),
        )
        .map_err(|error| error.to_string())
    }

    fn prepare(&self, request: Resolve) -> Result<Model, Error> {
        let configuration = Configuration::read(&request.connection)?;
        let reported = metadata::enrich(self.id, self.facts, request.model);
        let info = request
            .overrides
            .as_ref()
            .map_or_else(|| reported.clone(), |overrides| overrides.apply(&reported));
        let selection = Selection {
            provider: self.id,
            connection: &request.connection,
            base_url: &configuration.base_url,
            model: &info,
            overrides: request.overrides.as_ref(),
        };
        let route = route::resolve(&selection, self.facts)?;
        route.check_execution()?;
        let thinking_levels = request
            .overrides
            .as_ref()
            .and_then(|o| o.thinking_levels.clone())
            .or_else(|| info.thinking_levels.clone())
            .unwrap_or_default();
        let level = request
            .thinking_level
            .or_else(|| {
                request
                    .overrides
                    .as_ref()
                    .and_then(|o| o.default_thinking_level)
            })
            .or(info
                .default_thinking_level
                .filter(|level| thinking_levels.contains(level)));
        if level.is_some_and(|level| !thinking_levels.contains(&level)) {
            return Err(Error::Invalid("unsupported model thinking level".into()));
        }
        let options = options::resolve(&selection, self.facts, level, &route)?;
        let output = output::resolve(&selection, route.wire, &options)?;
        let adapter = request
            .overrides
            .as_ref()
            .and_then(|o| o.adapter.clone())
            .unwrap_or_else(|| {
                match route.wire {
                    Wire::OpenaiChat => "chat-completions",
                    Wire::OpenaiResponses => "responses",
                    Wire::AnthropicMessages => "anthropic-messages",
                }
                .into()
            });
        let model = Model {
            adapter,
            protocol: route.kind,
            base_url: route.base_url,
            info,
            thinking_levels,
            provider_options: options,
            main_output_limit: output,
        };
        model.validate()?;
        Ok(model)
    }
}

impl Provider for ApiProvider {
    fn resolve(&self, request: Resolve) -> BoxFuture<'_, Result<Model, Error>> {
        Box::pin(async move { self.prepare(request) })
    }

    fn authorize(
        &self,
        connection: Connection,
        credential: Option<maka_runtime::provider::Credential>,
        _: String,
    ) -> BoxFuture<'_, Result<maka_plugins::model::Credentials, Error>> {
        Box::pin(async move {
            Configuration::read(&connection)?;
            authentication::authorize(self.facts.auth_kind, credential)
        })
    }

    fn authenticate(
        &self,
        request: maka_plugins::provider::authentication::Authenticate,
        _: maka_plugins::provider::Context,
    ) -> BoxFuture<'_, Result<maka_runtime::provider::Credential, Error>> {
        Box::pin(async move {
            Configuration::read(&request.connection)?;
            authentication::authenticate(request)
        })
    }

    fn discover(
        &self,
        request: maka_plugins::provider::Discovery,
        context: maka_plugins::provider::Context,
    ) -> BoxFuture<'_, Result<Vec<ModelInfo>, Error>> {
        Box::pin(async move { effects::discover(self, request, context).await })
    }

    fn verify(
        &self,
        request: maka_plugins::provider::Verification,
        context: maka_plugins::provider::Context,
    ) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move { effects::verify(self, request, context).await })
    }
}
