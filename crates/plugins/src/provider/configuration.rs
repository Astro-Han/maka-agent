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

use super::{Error, authentication::Method};
use maka_runtime::{
    configuration::{ModelInfo, ModelOverride},
    execution::ThinkingLevel,
    model::request::ProviderKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Persistent user configuration is opaque to other providers, not a credential.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub revision: u64,
    pub configuration: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    pub label: String,
    pub configuration_schema: Value,
    pub configuration_defaults: Value,
    pub authentication: Vec<Method>,
    pub discovery: bool,
}

impl Descriptor {
    pub fn validate(&self) -> Result<(), Error> {
        if self.label.trim().is_empty()
            || self.label.len() > 256
            || self.label.chars().any(char::is_control)
            || !self.configuration_schema.is_object()
            || !self.configuration_defaults.is_object()
            || self.authentication.len() > 16
        {
            return Err(Error::Invalid(
                "provider descriptor exceeds its bounds".into(),
            ));
        }
        let bytes = serde_json::to_vec(self).map_err(|e| Error::Invalid(e.to_string()))?;
        if bytes.len() > 64 * 1024 {
            return Err(Error::Invalid("provider descriptor exceeds 64 KiB".into()));
        }
        let mut methods = std::collections::HashSet::new();
        for method in &self.authentication {
            method.validate()?;
            if !methods.insert(&method.id) {
                return Err(Error::Invalid("duplicate authentication method".into()));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resolve {
    pub connection: Connection,
    pub model: ModelInfo,
    pub overrides: Option<ModelOverride>,
    pub thinking_level: Option<ThinkingLevel>,
}

/// Resolved protocol settings. No prompt, tools or credential writes are exposed.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model {
    pub adapter: String,
    pub protocol: ProviderKind,
    pub base_url: String,
    pub info: ModelInfo,
    pub thinking_levels: Vec<ThinkingLevel>,
    pub provider_options: Value,
}

impl Model {
    pub fn validate(&self) -> Result<(), Error> {
        crate::name(&self.adapter).map_err(|e| Error::Invalid(e.to_string()))?;
        maka_runtime::configuration::validation::normalize_base_url(Some(&self.base_url), None)
            .map_err(Error::Invalid)?
            .ok_or_else(|| Error::Invalid("provider endpoint is empty".into()))?;
        self.info.validate().map_err(Error::Invalid)?;
        if !self.provider_options.is_object() && !self.provider_options.is_null() {
            return Err(Error::Invalid("provider options must be an object".into()));
        }
        for (index, level) in self.thinking_levels.iter().enumerate() {
            if self.thinking_levels[..index].contains(level) {
                return Err(Error::Invalid("duplicate thinking level".into()));
            }
        }
        Ok(())
    }
}
