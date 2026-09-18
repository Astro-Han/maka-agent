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

/// External tool activity is observation, never a request to dispatch a Host tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Output {
    OutputDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolStart {
        tool_call_id: String,
        name: String,
        input: Value,
    },
    ToolProgress {
        tool_call_id: String,
        text: String,
    },
    ToolResult {
        tool_call_id: String,
        text: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl Output {
    pub fn validate(&self) -> Result<(), &'static str> {
        let (id, text) = match self {
            Self::OutputDelta { text } | Self::ThinkingDelta { text } => (None, Some(text)),
            Self::ToolStart {
                tool_call_id,
                name,
                input,
            } => {
                opaque(name, 256)?;
                if serde_json::to_vec(input)
                    .map_err(|_| "invalid executor tool input")?
                    .len()
                    > 64 * 1024
                {
                    return Err("executor tool input exceeds 64 KiB");
                }
                (Some(tool_call_id), None)
            }
            Self::ToolProgress { tool_call_id, text }
            | Self::ToolResult {
                tool_call_id, text, ..
            } => (Some(tool_call_id), Some(text)),
        };
        if let Some(id) = id {
            opaque(id, 1024)?;
        }
        if text.is_some_and(|text| text.len() > 64 * 1024 || text.contains('\0')) {
            return Err("invalid executor text or chunk exceeds 64 KiB");
        }
        Ok(())
    }
}

fn opaque(value: &str, limit: usize) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err("invalid executor activity identity");
    }
    Ok(())
}
