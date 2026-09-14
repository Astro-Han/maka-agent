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

use crate::attachment::StorageRef;
use crate::capability::CallResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod durable;
mod mcp;
mod media;
pub(crate) use durable::encode_raw_tool_result;
pub use durable::{
    DURABLE_TOOL_PROJECTION_FAILURE_MESSAGE, DurableToolProjection, MAX_RAW_TOOL_RESULT_BYTES,
    MAX_RAW_TOOL_RESULT_JSON_DEPTH, ProjectionPart, RawToolResultRef, decode_raw_tool_result,
};
pub use media::ToolSuccess;

/// Live successful output. Durable evidence and projection are encoded separately.
/// JSON with MCP-looking fields is still JSON unless its executor says otherwise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", content = "value", rename_all = "snake_case")]
pub enum ToolOutput {
    Json(Value),
    Mcp(CallResult),
    Image(ImageOutput),
    Text(String),
}

/// Host-issued image evidence keeps bytes out of the canonical execution log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageOutput {
    pub mime_type: String,
    #[serde(rename = "ref")]
    pub reference: StorageRef,
}

impl ImageOutput {
    fn to_json(&self) -> Value {
        serde_json::json!({"kind":"image","mimeType":self.mime_type,"ref":self.reference})
    }
}

impl From<Value> for ToolOutput {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

impl ToolOutput {
    /// JavaScript and presentation see the original result, not model clipping.
    pub fn into_json(self) -> Value {
        match self {
            Self::Json(value) => value,
            Self::Mcp(result) => {
                serde_json::to_value(result).expect("MCP evidence contains only JSON values")
            }
            Self::Image(result) => result.to_json(),
            Self::Text(text) => serde_json::json!({"kind":"text","text":text}),
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::Json(value) => value.clone(),
            Self::Mcp(result) => {
                serde_json::to_value(result).expect("MCP evidence contains only JSON values")
            }
            Self::Image(result) => result.to_json(),
            Self::Text(text) => serde_json::json!({"kind":"text","text":text}),
        }
    }
}
