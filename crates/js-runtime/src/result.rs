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

use maka_runtime::tools::ToolError;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum CellAbort {
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("cell cancelled")]
    Cancelled,
    #[error("cell executor failed: {0}")]
    Internal(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CellDiagnosticKind {
    ParseError,
    ExecutionError,
    UnknownTool,
    LimitExceeded,
    ToolFailure,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CellDiagnostic {
    pub kind: CellDiagnosticKind,
    pub message: String,
}

impl CellDiagnostic {
    pub(crate) fn new(kind: CellDiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub(crate) fn limit(message: impl Into<String>) -> Self {
        Self::new(CellDiagnosticKind::LimitExceeded, message)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCall {
    pub index: usize,
    pub name: String,
}

#[derive(Debug)]
pub enum CellResult {
    Success {
        value: Value,
        tool_calls: Vec<ToolCall>,
    },
    Failure {
        error: CellDiagnostic,
        tool_calls: Vec<ToolCall>,
    },
}

impl Serialize for CellResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("CellResult", 3)?;
        match self {
            Self::Success { value, tool_calls } => {
                state.serialize_field("ok", &true)?;
                state.serialize_field("value", value)?;
                state.serialize_field("toolCalls", tool_calls)?;
            }
            Self::Failure { error, tool_calls } => {
                state.serialize_field("ok", &false)?;
                state.serialize_field("error", error)?;
                state.serialize_field("toolCalls", tool_calls)?;
            }
        }
        state.end()
    }
}

pub(crate) fn diagnostic_fits(calls: &[ToolCall], cap: usize) -> bool {
    // execution_error is the longest diagnostic kind. Reserve its empty envelope.
    serde_json::to_vec(&CellResult::Failure {
        error: CellDiagnostic::new(CellDiagnosticKind::ExecutionError, ""),
        tool_calls: calls.to_vec(),
    })
    .unwrap()
    .len()
        <= cap
}

pub(crate) fn bounded(
    result: Result<Value, CellDiagnostic>,
    calls: Vec<ToolCall>,
    cap: usize,
) -> CellResult {
    let mut envelope = match result {
        Ok(value) => CellResult::Success {
            value,
            tool_calls: calls,
        },
        Err(error) => CellResult::Failure {
            error,
            tool_calls: calls,
        },
    };
    if serde_json::to_vec(&envelope).unwrap().len() <= cap {
        return envelope;
    }
    if let CellResult::Success { tool_calls, .. } = envelope {
        envelope = CellResult::Failure {
            error: CellDiagnostic::limit("result bytes"),
            tool_calls,
        };
    }
    let CellResult::Failure { error, .. } = &mut envelope else {
        unreachable!()
    };
    let message = std::mem::take(&mut error.message);
    // Admission reserves the empty diagnostic; count JSON escaping once so a
    // very large thrown string cannot trigger quadratic reserialization.
    let available = cap - serde_json::to_vec(&envelope).unwrap().len();
    let mut used = 0;
    let mut end = 0;
    for (index, character) in message.char_indices() {
        let bytes = match character {
            '"' | '\\' | '\u{8}' | '\t' | '\n' | '\u{c}' | '\r' => 2,
            '\u{0}'..='\u{1f}' => 6,
            _ => character.len_utf8(),
        };
        if used + bytes > available {
            break;
        }
        used += bytes;
        end = index + character.len_utf8();
    }
    let CellResult::Failure { error, .. } = &mut envelope else {
        unreachable!()
    };
    error.message = message[..end].to_owned();
    envelope
}
