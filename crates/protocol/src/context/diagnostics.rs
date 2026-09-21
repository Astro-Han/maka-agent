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

use crate::{ProtocolError, Result, codec, turn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextDiagnosticsInput {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ContextDiagnosticsResult {
    Unavailable {
        reason: ContextDiagnosticsUnavailableReason,
    },
    Available {
        provider_id: String,
        model_id: String,
        completed_at: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_read_input_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        composition: Option<Box<ContextDiagnosticsComposition>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        compaction: Option<ContextDiagnosticsCompaction>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDiagnosticsUnavailableReason {
    NoCompletedRequest,
    TraceUnavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextDiagnosticsComposition {
    pub segments: Vec<ContextDiagnosticsSegment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ContextDiagnosticsTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_tools: Option<ContextDiagnosticsRemainder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlabelled_tool_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDiagnosticsSegment {
    pub kind: ContextDiagnosticsSegmentKind,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDiagnosticsSegmentKind {
    SystemInstructions,
    ToolDefinitions,
    Messages,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDiagnosticsTool {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDiagnosticsRemainder {
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ContextDiagnosticsCompaction {
    History {
        phase: ContextDiagnosticsPhase,
        event_count: u64,
        turn_count: u64,
        estimated_tokens: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDiagnosticsPhase {
    PreTurn,
    MidTurn,
}

pub fn decode_context_diagnostics_input(value: &Value) -> Result<ContextDiagnosticsInput> {
    let input: ContextDiagnosticsInput = turn::decode(value)?;
    turn::entity(&input.session_id)?;
    Ok(input)
}

pub fn decode_context_diagnostics_result(value: &Value) -> Result<ContextDiagnosticsResult> {
    // The shared decoder rejects explicit null and normalizes safe integral JSON
    // numbers (including 1.0) before serde checks the closed nested shapes.
    let result: ContextDiagnosticsResult = turn::decode(value)?;
    if let ContextDiagnosticsResult::Available {
        context_window,
        composition,
        ..
    } = &result
    {
        codec::string(&value["providerId"], "providerId", 512)?;
        codec::string(&value["modelId"], "modelId", 512)?;
        if *context_window == Some(0) {
            return Err(ProtocolError::invalid("Invalid contextWindow"));
        }
        if let Some(composition) = composition {
            if composition.segments.len() > 4
                || composition
                    .tools
                    .as_ref()
                    .is_some_and(|tools| tools.len() > 256)
            {
                return Err(ProtocolError::invalid(
                    "Invalid context diagnostics composition",
                ));
            }
            for tool in composition.tools.iter().flatten() {
                codec::string(&Value::String(tool.name.clone()), "tool name", 512)?;
            }
        }
    }
    Ok(result)
}
