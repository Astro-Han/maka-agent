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

//! Canonical interaction facts. The store owns append-only outcome commitment.
use serde::{Deserialize, Serialize};
mod decode;
mod question;
mod validation;
use crate::capability::{FormField, FormRequester, FormResult};
pub use question::{InteractionQuestion, QuestionOption};
pub use validation::{MAX_SAFE_INTEGER, entity_id};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantCapability {
    Browser,
    ComputerUse,
    DesktopMcp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum GrantScope {
    BrowserOrigin {
        origin: String,
    },
    Capability {},
    McpTool {
        server_id: String,
        tool_name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantTarget {
    pub provider_id: String,
    pub contract_id: String,
    pub server_id: String,
    pub tool_name: String,
    pub capability: GrantCapability,
    pub scope: GrantScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosureReason {
    TurnStopped,
    TurnTerminal,
    ProducerCancelled,
    TimedOut,
    HostRestarted,
    ProviderDisconnected,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum InteractionRequest {
    Question {
        tool_use_id: String,
        questions: Vec<InteractionQuestion>,
    },
    Form {
        tool_use_id: String,
        message: String,
        requester: FormRequester,
        fields: Vec<FormField>,
    },
    ClientCapability {
        tool_use_id: String,
        target: GrantTarget,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionAnswer {
    Question {
        answers: Vec<Option<String>>,
    },
    Form {
        #[serde(flatten)]
        result: FormResult,
    },
    ClientCapability {
        decision: Decision,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum InteractionOutcome {
    QuestionAnswer {
        answers: Vec<Option<String>>,
        committed_at: u64,
    },
    FormAnswer {
        #[serde(flatten)]
        result: FormResult,
        committed_at: u64,
    },
    ClientCapabilityDecision {
        decision: Decision,
        committed_at: u64,
    },
    Closure {
        reason: ClosureReason,
        committed_at: u64,
    },
}

impl InteractionOutcome {
    pub fn committed_at(&self) -> u64 {
        match self {
            Self::QuestionAnswer { committed_at, .. }
            | Self::FormAnswer { committed_at, .. }
            | Self::ClientCapabilityDecision { committed_at, .. }
            | Self::Closure { committed_at, .. } => *committed_at,
        }
    }
}

impl InteractionAnswer {
    pub fn into_outcome(self, committed_at: u64) -> InteractionOutcome {
        match self {
            Self::Question { answers } => InteractionOutcome::QuestionAnswer {
                answers,
                committed_at,
            },
            Self::ClientCapability { decision } => InteractionOutcome::ClientCapabilityDecision {
                decision,
                committed_at,
            },
            Self::Form { result } => InteractionOutcome::FormAnswer {
                result,
                committed_at,
            },
        }
    }

    pub fn matches_outcome(&self, outcome: &InteractionOutcome) -> bool {
        match (self, outcome) {
            (
                Self::Question { answers: left },
                InteractionOutcome::QuestionAnswer { answers: right, .. },
            ) => left == right,
            (
                Self::ClientCapability { decision: left },
                InteractionOutcome::ClientCapabilityDecision {
                    decision: right, ..
                },
            ) => left == right,
            (Self::Form { result: left }, InteractionOutcome::FormAnswer { result: right, .. }) => {
                left == right
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InteractionRecord {
    pub session_id: String,
    pub turn_id: String,
    pub run_id: String,
    pub request_id: String,
    pub created_at: u64,
    pub request: InteractionRequest,
    pub outcome: Option<InteractionOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionGrant {
    pub session_id: String,
    pub target: GrantTarget,
    pub granted_at: u64,
}
