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

use super::{bytes, ensure, entity, text};
use crate::{Result, codec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use maka_runtime::message::{TurnOrchestration, TurnOrchestrationSource};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSnapshot {
    pub session_id: String,
    pub turn_id: String,
    pub run_id: String,
    #[serde(flatten)]
    pub state: TurnState,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum TurnState {
    Admitted(LiveTurn),
    Created(LiveTurn),
    Running(LiveTurn),
    WaitingForUser(LiveTurn),
    Completed {
        terminal_event_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_compaction_outcome: Option<ContextCompactionOutcome>,
    },
    Failed {
        terminal_event_id: String,
        failure_class: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure_message: Option<String>,
    },
    Cancelled {
        terminal_event_id: String,
        abort_source: String,
    },
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTurn {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_retry: Option<TurnProviderRetry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_execution_kind: Option<RootExecutionKind>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootExecutionKind {
    ContextCompact,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ContextCompactionOutcome {
    Compacted { checkpoint_id: String },
    Unchanged { reason: String },
    Failed { reason: String },
}
impl ContextCompactionOutcome {
    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::Compacted { checkpoint_id } => entity(checkpoint_id),
            Self::Unchanged { reason } | Self::Failed { reason } => text(reason, 256),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "phase",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TurnProviderRetry {
    Scheduled {
        attempt: u64,
        max_attempts: u64,
        delay_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        ts: Option<u64>,
        reason: ProviderRetryReason,
    },
    Started {
        attempt: u64,
        max_attempts: u64,
        reason: ProviderRetryReason,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRetryReason {
    Network,
    ProviderCapacity,
    ProviderUnavailable,
    StreamTruncated,
    RateLimit,
    Timeout,
    Unknown,
}
impl TurnSnapshot {
    pub(super) fn validate(&self, value: &Value) -> Result<()> {
        entity(&self.session_id)?;
        entity(&self.turn_id)?;
        entity(&self.run_id)?;
        let mut required = vec!["sessionId", "turnId", "runId", "status"];
        let optional: &[&str] = match &self.state {
            TurnState::Admitted(live)
            | TurnState::Created(live)
            | TurnState::Running(live)
            | TurnState::WaitingForUser(live) => {
                if let Some(retry) = &live.provider_retry {
                    let (attempt, max) = match retry {
                        TurnProviderRetry::Scheduled {
                            attempt,
                            max_attempts,
                            ..
                        }
                        | TurnProviderRetry::Started {
                            attempt,
                            max_attempts,
                            ..
                        } => (*attempt, *max_attempts),
                    };
                    ensure(
                        attempt > 0 && attempt <= max,
                        "Invalid provider retry attempt",
                    )?;
                }
                &["providerRetry", "rootExecutionKind"]
            }
            TurnState::Completed {
                terminal_event_id,
                context_compaction_outcome,
            } => {
                required.push("terminalEventId");
                text(terminal_event_id, 128)?;
                if let Some(outcome) = context_compaction_outcome {
                    outcome.validate()?;
                }
                &["contextCompactionOutcome"]
            }
            TurnState::Failed {
                terminal_event_id,
                failure_class,
                failure_message,
            } => {
                required.extend(["terminalEventId", "failureClass"]);
                text(terminal_event_id, 128)?;
                text(failure_class, 128)?;
                if let Some(message) = failure_message {
                    bytes(message, 256, false)?;
                }
                &["failureMessage"]
            }
            TurnState::Cancelled {
                terminal_event_id,
                abort_source,
            } => {
                required.extend(["terminalEventId", "abortSource"]);
                text(terminal_event_id, 128)?;
                text(abort_source, 128)?;
                &[]
            }
        };
        codec::shaped(codec::record(value, "Turn snapshot")?, &required, optional)
    }
}
