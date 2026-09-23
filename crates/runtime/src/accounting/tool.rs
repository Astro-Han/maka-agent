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

use crate::{
    event::Invocation,
    execution::ModelBinding,
    tool_call::{RejectionKind, ToolCallIdentity},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    Error,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ToolResult {
    /// No effect was admitted, so no execution duration is reported.
    Rejected { reason: RejectionKind },
    Settled {
        started_at: f64,
        outcome: ToolStatus,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolAttempt {
    pub request_id: String,
    pub invocation: Invocation,
    pub call: ToolCallIdentity,
    pub name: String,
    pub binding: Option<ModelBinding>,
    pub completed_at: f64,
    pub result: ToolResult,
}
impl ToolAttempt {
    pub fn latency_ms(&self) -> Option<f64> {
        match &self.result {
            ToolResult::Rejected { .. } => None,
            ToolResult::Settled { started_at, .. } => {
                Some((self.completed_at - started_at).max(0.0))
            }
        }
    }
}
