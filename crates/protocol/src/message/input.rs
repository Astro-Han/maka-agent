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

use super::{Placement, ensure, epoch, identities};
use crate::{
    Operation, ProtocolError, Result,
    turn::{self, MessageContent, TurnOrchestration},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Input {
    Submit(Box<SubmitInput>),
    Query(QueryInput),
    ExecutionQuery(QueryInput),
    Retract(RetractInput),
    RetractEntry(RetractEntryInput),
    Promote(PromoteInput),
    Update(UpdateInput),
    Reorder(ReorderInput),
    Interrupt(InterruptInput),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitInput {
    pub origin_host_epoch: String,
    pub session_id: String,
    pub message_id: String,
    pub content: MessageContent,
    pub placement: Placement,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_orchestration: Option<TurnOrchestration>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryInput {
    pub session_id: String,
    pub message_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetractInput {
    pub origin_host_epoch: String,
    pub session_id: String,
    pub retract_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetractEntryInput {
    pub origin_host_epoch: String,
    pub session_id: String,
    pub entry_id: String,
    pub retract_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromoteInput {
    pub origin_host_epoch: String,
    pub session_id: String,
    pub entry_id: String,
    pub promote_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateInput {
    pub origin_host_epoch: String,
    pub session_id: String,
    pub entry_id: String,
    pub update_id: String,
    pub expected_queue_revision: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReorderInput {
    pub origin_host_epoch: String,
    pub session_id: String,
    pub reorder_id: String,
    pub entry_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InterruptInput {
    pub origin_host_epoch: String,
    pub session_id: String,
    pub interrupt_id: String,
    pub turn_id: String,
    pub run_id: String,
}

pub fn decode_input(operation: Operation, value: &Value) -> Result<Input> {
    use Operation::*;
    let input = match operation {
        TurnMessageSubmit => {
            let mut input: SubmitInput = turn::decode(value)?;
            epoch(&input.origin_host_epoch)?;
            turn::entity(&input.session_id)?;
            turn::entity(&input.message_id)?;
            let ids = input.skill_ids.as_deref().unwrap_or_default();
            turn::validate_skill_ids(ids)?;
            input.content.validate_admission(!ids.is_empty())?;
            ensure(
                (ids.is_empty() && input.turn_orchestration.is_none())
                    || input.placement == Placement::CurrentTurn,
                "Exact-Turn intent requires current_turn placement",
            )?;
            if ids.is_empty() {
                input.skill_ids = None;
            }
            Input::Submit(Box::new(input))
        }
        TurnMessageQuery | TurnMessageExecutionQuery => {
            let input: QueryInput = turn::decode(value)?;
            turn::entity(&input.session_id)?;
            identities(&input.message_ids)?;
            if operation == TurnMessageQuery {
                Input::Query(input)
            } else {
                Input::ExecutionQuery(input)
            }
        }
        QueueRetract => {
            let input: RetractInput = turn::decode(value)?;
            command(
                &input.origin_host_epoch,
                &input.session_id,
                &[&input.retract_id],
            )?;
            Input::Retract(input)
        }
        QueueEntryRetract => {
            let input: RetractEntryInput = turn::decode(value)?;
            command(
                &input.origin_host_epoch,
                &input.session_id,
                &[&input.entry_id, &input.retract_id],
            )?;
            Input::RetractEntry(input)
        }
        QueueEntryPromote => {
            let input: PromoteInput = turn::decode(value)?;
            command(
                &input.origin_host_epoch,
                &input.session_id,
                &[&input.entry_id, &input.promote_id],
            )?;
            Input::Promote(input)
        }
        QueueEntryUpdate => {
            let input: UpdateInput = turn::decode(value)?;
            command(
                &input.origin_host_epoch,
                &input.session_id,
                &[&input.entry_id, &input.update_id],
            )?;
            let blank = input
                .text
                .chars()
                .all(|c| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}');
            ensure(
                input.text.len() <= 48 * 1024 && !blank,
                "Invalid message text",
            )?;
            Input::Update(input)
        }
        QueueEntriesReorder => {
            let input: ReorderInput = turn::decode(value)?;
            command(
                &input.origin_host_epoch,
                &input.session_id,
                &[&input.reorder_id],
            )?;
            identities(&input.entry_ids)?;
            Input::Reorder(input)
        }
        TurnInterrupt => {
            let input: InterruptInput = turn::decode(value)?;
            command(
                &input.origin_host_epoch,
                &input.session_id,
                &[&input.interrupt_id, &input.turn_id, &input.run_id],
            )?;
            Input::Interrupt(input)
        }
        _ => return Err(ProtocolError::invalid("Not a message operation")),
    };
    Ok(input)
}

fn command(origin: &str, session: &str, ids: &[&str]) -> Result<()> {
    epoch(origin)?;
    turn::entity(session)?;
    for id in ids {
        turn::entity(id)?;
    }
    Ok(())
}
