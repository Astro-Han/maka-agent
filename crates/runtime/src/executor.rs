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

use serde::{Deserialize, Serialize};
mod output;
pub use output::Output;

/// Executor-owned model selection, independent of Host model connections.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<crate::execution::ThinkingLevel>,
}
impl Settings {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.thinking_level.is_none()
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.model.as_ref().is_some_and(|model| {
            model.trim().is_empty() || model.len() > 512 || model.chars().any(char::is_control)
        }) {
            return Err("executor model must contain 1–512 UTF-8 bytes without control characters");
        }
        Ok(())
    }
}

/// The concrete implementation accepted for an external execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub executor_id: ExecutorId,
    pub package_id: String,
    pub entry_id: String,
    pub activation: String,
}

impl Binding {
    pub fn validate(&self) -> Result<(), &'static str> {
        ExecutorId::try_from(self.package_id.clone())?;
        ExecutorId::try_from(self.entry_id.clone())?;
        crate::interaction::entity_id(&self.activation)?;
        Ok(())
    }
}

/// A plugin-contributed execution backend identity, not a model or connection ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExecutorId(String);

impl ExecutorId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ExecutorId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > 128
            || !value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return Err("invalid executor ID");
        }
        Ok(Self(value))
    }
}

impl From<ExecutorId> for String {
    fn from(value: ExecutorId) -> Self {
        value.0
    }
}
