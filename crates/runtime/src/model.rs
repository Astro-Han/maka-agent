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
use serde_json::Value;

/// Provider metadata is an open adapter-owned object, merged without losing
/// nested reasoning/signature evidence as subsequent observations arrive.
pub fn merge_provider_options(target: &mut Option<Value>, next: Option<Value>) {
    fn overlay(target: &mut Value, next: Value) {
        if let (Some(a), Some(b)) = (target.as_object_mut(), next.as_object()) {
            for (key, value) in b {
                overlay(a.entry(key.clone()).or_insert(Value::Null), value.clone());
            }
        } else {
            *target = next;
        }
    }
    if let Some(next) = next {
        overlay(target.get_or_insert(Value::Null), next);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelFinishReason {
    Stop,
    ToolCalls,
    Length,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextKind {
    Text,
    Thinking,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub provider_options: Option<Value>,
    pub provider_executed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelPart {
    Text {
        text_kind: TextKind,
        text: String,
        provider_options: Option<Value>,
    },
    ToolCall {
        call: ModelToolCall,
    },
    ToolResult {
        id: String,
        name: String,
        output: Value,
        is_error: bool,
        provider_options: Option<Value>,
    },
}

/// Runtime-owned stream vocabulary; provider chunk names never enter storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ModelEvent {
    PartStarted {
        id: String,
        text_kind: TextKind,
        provider_options: Option<Value>,
    },
    PartDelta {
        id: String,
        text: String,
        provider_options: Option<Value>,
    },
    PartFinished {
        id: String,
        provider_options: Option<Value>,
    },
    ToolCall(ModelToolCall),
    ProviderToolResult {
        id: String,
        name: String,
        output: Value,
        is_error: bool,
        provider_options: Option<Value>,
    },
    ResponseMetadata {
        id: Option<String>,
        model: Option<String>,
        timestamp: Option<String>,
    },
    Finished {
        reason: ModelFinishReason,
        usage: ModelUsage,
        provider_options: Option<Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelStep {
    pub parts: Vec<ModelPart>,
    pub finish_reason: ModelFinishReason,
    pub usage: ModelUsage,
    pub provider_options: Option<Value>,
    pub response_id: Option<String>,
    pub model: Option<String>,
    pub timestamp: Option<String>,
}

impl ModelStep {
    pub fn tool_calls(&self) -> impl Iterator<Item = &ModelToolCall> {
        self.parts.iter().filter_map(|part| match part {
            ModelPart::ToolCall { call } => Some(call),
            _ => None,
        })
    }
}
