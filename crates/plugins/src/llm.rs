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

pub use maka_runtime::model::ModelGeneration;
use serde::{Deserialize, Serialize};

/// A user-selected model name, not provider configuration or execution authority.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Selection {
    Default,
    Named {
        connection_slug: String,
        model: String,
    },
}
impl Selection {
    pub fn validate(&self) -> Result<(), crate::Error> {
        if let Self::Named {
            connection_slug,
            model,
        } = self
        {
            crate::name(connection_slug)?;
            crate::name(model)?;
        }
        Ok(())
    }
}

pub trait Models: Send + Sync {
    /// Bounded, non-secret chat model choices. Refine the query when incomplete.
    /// Discovery does not grant execution authority or promise provider readiness.
    fn search(
        &self,
        query: Search,
    ) -> futures_util::future::BoxFuture<'_, Result<Choices, crate::Error>>;
    /// Resolve an enabled model without exposing credentials, endpoints, or overlays.
    /// Admission still validates the selected binding and its current permissions.
    fn resolve(
        &self,
        selection: Selection,
    ) -> futures_util::future::BoxFuture<
        '_,
        Result<Option<maka_runtime::execution::ModelBinding>, crate::Error>,
    >;

    fn generate(
        &self,
        call: crate::call::Scope,
        input: Generate,
    ) -> futures_util::future::BoxFuture<'_, Result<ModelGeneration, maka_runtime::tools::ToolError>>;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Search {
    #[serde(default)]
    pub query: String,
}
impl Search {
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.query.len() > 512 || self.query.chars().any(char::is_control) {
            return Err(crate::Error::Invalid("invalid model search query".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Choice {
    pub model: maka_runtime::execution::ModelBinding,
    pub connection_name: String,
    pub display_name: String,
    pub thinking_levels: Vec<maka_runtime::execution::ThinkingLevel>,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Choices {
    pub revision: u64,
    pub models: Vec<Choice>,
    pub complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Generate {
    pub prompt: String,
    pub system: Option<String>,
    pub max_output_tokens: Option<u64>,
}
impl Generate {
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.prompt.trim().is_empty()
            || self
                .prompt
                .len()
                .saturating_add(self.system.as_ref().map_or(0, String::len))
                > 256 * 1024
            || self
                .max_output_tokens
                .is_some_and(|value| value == 0 || value > 9_007_199_254_740_991)
        {
            return Err(crate::Error::Invalid(
                "invalid or oversized model generation".into(),
            ));
        }
        Ok(())
    }
}
