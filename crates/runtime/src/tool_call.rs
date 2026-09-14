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
use sha2::{Digest, Sha256};

/// Stable Host tool-use identity shared by client callbacks and presentation.
/// Provider IDs remain step-scoped model evidence and may not be wire-safe.
pub fn tool_use_id(invocation_id: &str, operation_id: &str) -> String {
    let bytes =
        serde_json::to_vec(&["maka.tool-presentation.v1", invocation_id, operation_id]).unwrap();
    format!("tool_{:x}", Sha256::digest(bytes))
}

pub fn provider_result_id(event_id: &str, part_index: usize) -> String {
    format!("{event_id}_tool_result_{part_index}")
}

pub fn parse_provider_result_id(id: &str) -> Option<(&str, usize)> {
    let (event, index) = id.rsplit_once("_tool_result_")?;
    let index: usize = index.parse().ok()?;
    (provider_result_id(event, index) == id).then_some((event, index))
}

/// A known refusal before durable dispatch: no tool effect was admitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolRejection {
    #[error("tool is unavailable")]
    Unavailable,
    #[error("invalid tool input: {message}")]
    InvalidInput { message: String },
    #[error("tool policy denied: {message}")]
    PolicyDenied { message: String },
    #[error("tool preparation failed: {message}")]
    PreparationFailed { message: String },
    #[error("tool conflicts with exclusive step execution")]
    ExclusiveConflict,
    #[error("tool cancelled before dispatch; tool was not executed")]
    Cancelled,
}

/// A Tool Call is distinct from its durable host operation. Origin determines
/// model visibility; it is not an independent flag that can contradict ancestry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallIdentity {
    pub tool_call_id: String,
    pub origin: ToolOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolOrigin {
    Provider {
        step_id: String,
    },
    CodeMode {
        parent_operation_id: String,
        parent_tool_call_id: String,
    },
    Standalone,
}

impl ToolCallIdentity {
    pub fn provider(step_id: String, tool_call_id: String) -> Self {
        Self {
            tool_call_id,
            origin: ToolOrigin::Provider { step_id },
        }
    }

    pub fn standalone(tool_call_id: String) -> Self {
        Self {
            tool_call_id,
            origin: ToolOrigin::Standalone,
        }
    }
}
