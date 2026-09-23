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

use crate::{ProtocolError, Result, turn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const ERRORS: &[crate::OperationErrorCode] = &[
    crate::OperationErrorCode::HostNotReady,
    crate::OperationErrorCode::HostDraining,
    crate::OperationErrorCode::OperationUnavailable,
    crate::OperationErrorCode::InvalidRequest,
    crate::OperationErrorCode::NotFound,
    crate::OperationErrorCode::OperationConflict,
    crate::OperationErrorCode::PersistenceFailed,
    crate::OperationErrorCode::InternalFailure,
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub session_id: String,
    pub turn_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub message_id: String,
    pub content: turn::MessageContent,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub input_selections: maka_runtime::input::Selections,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_orchestration: Option<turn::TurnOrchestration>,
}

impl From<maka_runtime::message::EditableMessage> for Source {
    fn from(message: maka_runtime::message::EditableMessage) -> Self {
        let (input_selections, turn_orchestration) = message
            .intent
            .map(|intent| (intent.input_selections, intent.turn_orchestration))
            .unwrap_or_default();
        Self {
            message_id: message.message_id,
            content: message.content.into(),
            input_selections,
            turn_orchestration,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Output {
    pub session_id: String,
    pub turn_id: String,
    pub messages: Vec<Source>,
}

pub fn decode_input(value: &Value) -> Result<Input> {
    let input: Input = turn::decode(value)?;
    turn::entity(&input.session_id)?;
    turn::entity(&input.turn_id)?;
    Ok(input)
}

pub fn decode_output(value: &Value) -> Result<Output> {
    let output: Output = turn::decode(value)?;
    turn::entity(&output.session_id)?;
    turn::entity(&output.turn_id)?;
    if output.messages.is_empty() || output.messages.len() > 64 {
        return Err(ProtocolError::invalid("Invalid Turn source count"));
    }
    let mut seen = HashSet::new();
    for message in &output.messages {
        turn::entity(&message.message_id)?;
        if !seen.insert(&message.message_id) {
            return Err(ProtocolError::invalid("Duplicate Turn source"));
        }
        maka_runtime::input::validate_selections(&message.input_selections)
            .map_err(ProtocolError::invalid)?;
    }
    if serde_json::to_vec(&output)
        .map_err(|error| ProtocolError::invalid(error.to_string()))?
        .len()
        > 700 * 1024
    {
        return Err(ProtocolError::invalid("Turn sources exceed read capacity"));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn source_views_preserve_order_and_read_capacity_without_relaxing_admission() {
        let value = json!({"sessionId":"target", "turnId":"turn", "messages":[
            {"messageId":"first", "content":{"text":"\n".repeat(60 * 1024)}},
            {"messageId":"second", "content":{"text":"second", "quotes":[{"text":"q".repeat(40_000)}]}, "inputSelections":{"skills":["review"]}}
        ]});
        let sources = decode_output(&value).unwrap();
        assert_eq!(sources.messages[0].message_id, "first");
        assert_eq!(sources.messages[1].input_selections["skills"], ["review"]);
        assert!(
            sources.messages[0]
                .content
                .clone()
                .validate_admission(false)
                .is_err()
        );
        assert!(
            sources.messages[1]
                .content
                .clone()
                .validate_admission(false)
                .is_err()
        );
        let mut duplicate = value;
        duplicate["messages"][1]["messageId"] = json!("first");
        assert!(decode_output(&duplicate).is_err());
    }
}
