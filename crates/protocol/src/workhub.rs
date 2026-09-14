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

use crate::{Operation, ProtocolError, Result, codec, session::*};
use maka_runtime::workhub::COORDINATION_SESSION_ID;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod action;
mod candidates;
mod selection;
pub use action::{
    ActInput, ActResult, CreateContext, DelegationDisposition, LinkedProposal, Proposal,
    ResumeOutcome, RoutingProposal, decode_act,
};
pub use candidates::{Candidate, CandidatesResult, decode_candidates};
pub use selection::{SelectionInput, SelectionResult, decode_selection};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnswerInput {
    pub turn_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<crate::turn::AttachmentRef>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnResult {
    pub turn_id: String,
}

impl AnswerInput {
    pub fn content(&self) -> crate::turn::MessageContent {
        crate::turn::MessageContent {
            text: self.text.clone(),
            attachments: self.attachments.clone(),
            display_text: None,
            directory_references: None,
            quotes: None,
            inline_references: None,
        }
    }
}

pub fn decode_answer_input(value: &Value) -> Result<AnswerInput> {
    let input: AnswerInput = crate::turn::decode(value)?;
    crate::turn::entity(&input.turn_id)?;
    input.content().validate_admission(false)?;
    Ok(input)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveResult {
    pub session_id: String,
}

/// Only the model is writable through WorkHub authority.
pub fn decode_model_input(value: &Value) -> Result<SessionConfigurationUpdateInput> {
    codec::exact(
        codec::record(value, "WorkHub model configuration")?,
        &["expectedRevision", "modelTarget"],
    )?;
    decode_session_configuration_update_input(&json!({
        "sessionId": COORDINATION_SESSION_ID,
        "expectedRevision": value["expectedRevision"],
        "patch": {"modelTarget": value["modelTarget"]}
    }))
}

pub fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::WorkhubCoordinationResolve
            | Operation::WorkhubCoordinationQuery
            | Operation::WorkhubCoordinationConfigureModel
            | Operation::WorkhubCoordinationAnswer
            | Operation::WorkhubCoordinationCandidates
            | Operation::WorkhubCoordinationActFromTurn
            | Operation::WorkhubCoordinationSelectAndDelegate
    )
}

pub fn decode_input(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::WorkhubCoordinationResolve
        | Operation::WorkhubCoordinationQuery
        | Operation::WorkhubCoordinationCandidates => {
            codec::exact(codec::record(value, "WorkHub coordination")?, &[])?;
        }
        Operation::WorkhubCoordinationConfigureModel => {
            decode_model_input(value)?;
        }
        Operation::WorkhubCoordinationAnswer => {
            decode_answer_input(value)?;
        }
        Operation::WorkhubCoordinationActFromTurn => {
            decode_act(value)?;
        }
        Operation::WorkhubCoordinationSelectAndDelegate => {
            decode_selection(value)?;
        }
        _ => return Err(ProtocolError::invalid("Unknown WorkHub operation")),
    }
    Ok(value.clone())
}

pub fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::WorkhubCoordinationResolve => {
            let result: ResolveResult = crate::turn::decode(value)?;
            if result.session_id != COORDINATION_SESSION_ID {
                return Err(ProtocolError::invalid("Invalid WorkHub Session identity"));
            }
        }
        Operation::WorkhubCoordinationQuery => {
            if decode_session_catalog_item(value)?.id() != COORDINATION_SESSION_ID {
                return Err(ProtocolError::invalid("Invalid WorkHub Session identity"));
            }
        }
        Operation::WorkhubCoordinationConfigureModel => {
            let result = decode_session_update_result(value)?;
            if let SessionUpdateResult::Committed { session, .. } = result
                && session.id() != COORDINATION_SESSION_ID
            {
                return Err(ProtocolError::invalid("Invalid WorkHub Session identity"));
            }
        }
        Operation::WorkhubCoordinationAnswer => {
            let result: TurnResult = crate::turn::decode(value)?;
            crate::turn::entity(&result.turn_id)?;
        }
        Operation::WorkhubCoordinationCandidates => {
            decode_candidates(value)?;
        }
        Operation::WorkhubCoordinationActFromTurn => {
            let result: ActResult = crate::turn::decode(value)?;
            result.validate()?;
        }
        Operation::WorkhubCoordinationSelectAndDelegate => {
            let result: SelectionResult = crate::turn::decode(value)?;
            if let SelectionResult::Delegated { result } = result {
                result.validate()?;
            }
        }
        _ => return Err(ProtocolError::invalid("Unknown WorkHub operation")),
    }
    Ok(value.clone())
}
