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

use crate::{artifact::content_digest, tools::ToolDefinition};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Tool,
    PromptSection,
    PromptVariable,
    PromptContext,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRevision {
    pub kind: SourceKind,
    pub name: String,
    pub package_id: String,
    pub entry_id: String,
    pub activation: String,
    pub revision: String,
}

/// The immutable capability surface of one logical model step. Conversation
/// history retains its own canonical prefix evidence in ModelRequested.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestComposition {
    pub system_prompt: Option<String>,
    pub dynamic_context: Vec<String>,
    pub tool_catalog_digest: String,
    pub tools: Vec<ToolDefinition>,
    pub provider_options: Option<Value>,
    pub max_output_tokens: Option<u64>,
    pub sources: Vec<SourceRevision>,
}
impl RequestComposition {
    pub fn freeze(self) -> Result<FrozenComposition, &'static str> {
        if self
            .system_prompt
            .as_ref()
            .is_some_and(|text| text.len() > 64 * 1024)
            || self.dynamic_context.len() > 128
            || self.dynamic_context.iter().map(String::len).sum::<usize>() > 64 * 1024
            || self.tools.len() > 129
            || self.sources.len() > 512
            || !crate::archive::valid_projection_digest(&self.tool_catalog_digest)
        {
            return Err("invalid request composition limits");
        }
        let mut names = BTreeSet::new();
        for tool in &self.tools {
            if tool.name.is_empty() || tool.name.len() > 256 || !names.insert(&tool.name) {
                return Err("invalid request composition tools");
            }
        }
        for source in &self.sources {
            for value in [
                &source.name,
                &source.package_id,
                &source.entry_id,
                &source.activation,
                &source.revision,
            ] {
                if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
                    return Err("invalid request composition source");
                }
            }
        }
        let bytes =
            serde_json::to_vec(&self).map_err(|_| "invalid request composition encoding")?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("request composition exceeds 4 MiB");
        }
        Ok(FrozenComposition {
            digest: content_digest(&bytes),
            bytes: bytes.into_boxed_slice(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct FrozenComposition {
    digest: String,
    bytes: Box<[u8]>,
}
impl FrozenComposition {
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
