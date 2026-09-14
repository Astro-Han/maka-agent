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

use super::candidates::{text, workspace};
use crate::{ProtocolError, Result, session::WorkspaceTarget};
use maka_runtime::workhub::ActionId;
pub use maka_runtime::workhub::CreateDefaults;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActInput {
    pub turn_id: String,
    pub action_id: ActionId,
    pub proposal: Proposal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_set_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create: Option<CreateContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_work_defaults: Option<CreateDefaults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegation_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Proposal {
    Route(RoutingProposal),
    Linked(LinkedProposal),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "disposition",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RoutingProposal {
    DelegateExisting { candidate_ref: String },
    CreateNew { title: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LinkedProposal {
    Correct {
        replaces_action_id: ActionId,
        target: RoutingProposal,
    },
    Stop {
        expects: LinkedTarget,
    },
    Resume {
        resumes_action_id: ActionId,
        expects: LinkedTarget,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkedTarget {
    pub target_session_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateContext {
    pub workspace: WorkspaceTarget,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "disposition",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ActResult {
    Replace {
        replacement_disposition: DelegationDisposition,
        target_session_id: String,
        target_turn_id: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        steered: bool,
    },
    CreateNew {
        target_session_id: String,
        target_turn_id: String,
    },
    DelegateExisting {
        target_session_id: String,
        target_turn_id: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        steered: bool,
    },
    StopWork {
        outcome: maka_runtime::workhub::StopOutcome,
        target_session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_turn_id: Option<String>,
    },
    ResumeWork {
        outcome: ResumeOutcome,
        target_session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_turn_id: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeOutcome {
    ResumeStarted,
    AlreadyRunning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationDisposition {
    DelegateExisting,
    CreateNew,
}

impl ActResult {
    pub(super) fn validate(&self) -> Result<()> {
        if let Self::ResumeWork {
            outcome: ResumeOutcome::ResumeStarted,
            target_turn_id: None,
            ..
        } = self
        {
            return Err(invalid());
        }
        let (target_session_id, target_turn_id) = match self {
            Self::CreateNew {
                target_session_id,
                target_turn_id,
            }
            | Self::DelegateExisting {
                target_session_id,
                target_turn_id,
                ..
            }
            | Self::Replace {
                target_session_id,
                target_turn_id,
                ..
            } => (target_session_id, Some(target_turn_id)),
            Self::StopWork {
                target_session_id,
                target_turn_id,
                ..
            }
            | Self::ResumeWork {
                target_session_id,
                target_turn_id,
                ..
            } => (target_session_id, target_turn_id.as_ref()),
        };
        crate::turn::entity(target_session_id)?;
        if let Some(turn) = target_turn_id {
            crate::turn::entity(turn)?;
        }
        Ok(())
    }
}

pub fn decode_act(value: &Value) -> Result<ActInput> {
    let input: ActInput = crate::turn::decode(value)?;
    for field in [
        "candidateSetId",
        "create",
        "newWorkDefaults",
        "delegationText",
    ] {
        if value.get(field).is_some_and(Value::is_null) {
            return Err(invalid());
        }
    }
    crate::turn::entity(&input.turn_id)?;
    let route = match &input.proposal {
        Proposal::Route(route) => Some(route),
        Proposal::Linked(LinkedProposal::Correct { target, .. }) => Some(target),
        Proposal::Linked(LinkedProposal::Stop { expects }) => {
            crate::turn::entity(&expects.target_session_id)?;
            None
        }
        Proposal::Linked(LinkedProposal::Resume { expects, .. }) => {
            crate::turn::entity(&expects.target_session_id)?;
            None
        }
    };
    if let Some(delegation) = &input.delegation_text {
        text(delegation, 48 * 1024)?;
        if delegation.trim().is_empty() || route.is_none() {
            return Err(invalid());
        }
    }
    match route {
        Some(RoutingProposal::DelegateExisting { candidate_ref }) => {
            crate::turn::entity(candidate_ref)?;
            text(input.candidate_set_id.as_deref().ok_or_else(invalid)?, 96)?;
            if input.create.is_some() || input.new_work_defaults.is_some() {
                return Err(invalid());
            }
        }
        Some(RoutingProposal::CreateNew { title }) => {
            text(title, 512)?;
            workspace(&input.create.as_ref().ok_or_else(invalid)?.workspace)?;
            if input.candidate_set_id.is_some() {
                return Err(invalid());
            }
        }
        None => {
            if input.candidate_set_id.is_some()
                || input.create.is_some()
                || input.new_work_defaults.is_some()
            {
                return Err(invalid());
            }
        }
    }
    if let Some(defaults) = &input.new_work_defaults {
        for field in ["model", "executorId", "permissionMode"] {
            if value["newWorkDefaults"]
                .get(field)
                .is_some_and(Value::is_null)
            {
                return Err(invalid());
            }
        }
        if let Some(model) = defaults.model() {
            for field in [
                &model.llm_connection_id,
                &model.llm_connection_slug,
                &model.model,
            ] {
                if field.trim().is_empty() || field.encode_utf16().count() > 512 {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(input)
}
fn invalid() -> ProtocolError {
    ProtocolError::invalid("Invalid WorkHub action context")
}
