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

use super::{MAX_RESULT_BYTES, QueueEntry, encoded, ensure, identities, queue};
use crate::{
    Operation, ProtocolError, Result,
    turn::{self, SkillInvocationResult, TurnSnapshot},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Output {
    Submit(SubmitResult),
    Query(QueryResult),
    Executions(ExecutionQueryResult),
    Retract(RetractResult),
    Mutation(MutationResult),
    Interrupt(Box<InterruptResult>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "disposition",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SubmitResult {
    TurnStarted {
        turn_id: String,
        skill_invocation: SkillInvocationResult,
    },
    Blocked {
        skill_invocation: SkillInvocationResult,
    },
    Steering {
        skill_invocation: SkillInvocationResult,
        #[serde(skip_serializing_if = "Option::is_none")]
        queue_revision: Option<u64>,
    },
    Followup {
        skill_invocation: SkillInvocationResult,
        #[serde(skip_serializing_if = "Option::is_none")]
        queue_revision: Option<u64>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryResult {
    pub cancelled_message_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionQueryResult {
    pub resolutions: Vec<ExecutionResolution>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ExecutionResolution {
    Pending {
        message_id: String,
    },
    Cancelled {
        message_id: String,
    },
    Owned {
        message_id: String,
        turn_id: String,
        run_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetractResult {
    pub queue_revision: u64,
    pub retracted: Vec<QueueEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationResult {
    pub queue_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InterruptResult {
    pub queue_revision: u64,
    pub retracted: Vec<QueueEntry>,
    pub turn: TurnSnapshot,
}

pub fn decode_output(operation: Operation, value: &Value) -> Result<Output> {
    use Operation::*;
    let output = match operation {
        TurnMessageSubmit => {
            let output: SubmitResult = turn::decode(value)?;
            let skills = match &output {
                SubmitResult::TurnStarted {
                    turn_id,
                    skill_invocation,
                } => {
                    turn::entity(turn_id)?;
                    skill_invocation
                }
                SubmitResult::Blocked { skill_invocation } => {
                    ensure(
                        skill_invocation.loaded.is_empty() && !skill_invocation.failed.is_empty(),
                        "Blocked submission requires failed skills",
                    )?;
                    skill_invocation
                }
                SubmitResult::Steering {
                    skill_invocation, ..
                }
                | SubmitResult::Followup {
                    skill_invocation, ..
                } => skill_invocation,
            };
            skills.validate().map_err(ProtocolError::invalid)?;
            Output::Submit(output)
        }
        TurnMessageQuery => {
            let output: QueryResult = turn::decode(value)?;
            identities(&output.cancelled_message_ids)?;
            Output::Query(output)
        }
        TurnMessageExecutionQuery => {
            let output: ExecutionQueryResult = turn::decode(value)?;
            ensure(
                output.resolutions.len() <= super::MAX_ENTRIES,
                "Too many execution resolutions",
            )?;
            let mut seen = HashSet::new();
            for resolution in &output.resolutions {
                let message = match resolution {
                    ExecutionResolution::Pending { message_id }
                    | ExecutionResolution::Cancelled { message_id } => message_id,
                    ExecutionResolution::Owned {
                        message_id,
                        turn_id,
                        run_id,
                    } => {
                        turn::entity(turn_id)?;
                        turn::entity(run_id)?;
                        message_id
                    }
                };
                turn::entity(message)?;
                ensure(seen.insert(message), "Duplicate execution resolution")?;
            }
            Output::Executions(output)
        }
        QueueRetract => {
            let mut output: RetractResult = turn::decode(value)?;
            queue::retracted(&mut output.retracted)?;
            encoded(&output, MAX_RESULT_BYTES)?;
            Output::Retract(output)
        }
        QueueEntryRetract | QueueEntryPromote | QueueEntryUpdate | QueueEntriesReorder => {
            Output::Mutation(turn::decode(value)?)
        }
        TurnInterrupt => {
            let mut output: InterruptResult = turn::decode(value)?;
            output.turn = turn::decode_turn_snapshot(&value["turn"])?;
            queue::retracted(&mut output.retracted)?;
            encoded(&output, MAX_RESULT_BYTES)?;
            Output::Interrupt(Box::new(output))
        }
        _ => return Err(ProtocolError::invalid("Not a message operation")),
    };
    Ok(output)
}
