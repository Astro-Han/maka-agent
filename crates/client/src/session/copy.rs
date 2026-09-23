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

use crate::{Client, ClientError, RequestFailure};
use maka_protocol::{
    Operation, ProtocolError,
    session::{copy, sources},
};
use serde_json::{Value, json};

impl Client {
    pub async fn copy_session(&self, input: copy::Input) -> Result<copy::Output, RequestFailure> {
        let (operation, value) = wire(&input)
            .map_err(|e| RequestFailure::NotDispatched(ClientError::Protocol(e.to_string())))?;
        let output = self.request(operation, value).await?;
        copy::decode_output(&input, &output).map_err(|e| self.invalid_session(e))
    }

    /// Read an exact copy's receipt without creating/adopting a target. An absent
    /// receipt does not prove that an earlier request was never admitted.
    pub async fn query_session_copy(
        &self,
        expected: copy::Input,
    ) -> Result<copy::QueryResult, RequestFailure> {
        let output = self
            .request(
                Operation::SessionCopyQuery,
                json!({"targetSessionId":expected.target_session_id}),
            )
            .await?;
        let output = copy::decode_query_result(&output).map_err(|e| self.invalid_session(e))?;
        if output
            .receipt
            .as_ref()
            .is_some_and(|receipt| receipt.request != expected)
        {
            return Err(self.invalid_session(ProtocolError::invalid(
                "Session copy receipt changed its request",
            )));
        }
        Ok(output)
    }

    pub async fn abandon_session_revision(
        &self,
        input: copy::AbandonInput,
    ) -> Result<copy::AbandonOutput, RequestFailure> {
        let output = self
            .request(
                Operation::SessionRevisionAbandon,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        copy::decode_abandon_output(&input, &output).map_err(|e| self.invalid_session(e))
    }

    /// Return original inputs, including attachments and selections. A source
    /// read is not admission validation and must not be flattened into plain text.
    pub async fn session_turn_sources(
        &self,
        input: sources::Input,
    ) -> Result<sources::Output, RequestFailure> {
        let output = self
            .request(
                Operation::SessionSourcesQuery,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output = sources::decode_output(&output).map_err(|e| self.invalid_session(e))?;
        if output.session_id != input.session_id || output.turn_id != input.turn_id {
            return Err(self.invalid_session(ProtocolError::invalid(
                "Session sources do not match the selected Turn",
            )));
        }
        Ok(output)
    }
}

fn wire(input: &copy::Input) -> Result<(Operation, Value), ProtocolError> {
    let (operation, turn, side) = match &input.purpose {
        copy::Purpose::Branch {
            turn_id: Some(turn),
            side_conversation,
        } => (
            Operation::SessionBranchCreate,
            Some(turn),
            *side_conversation,
        ),
        copy::Purpose::EmptySideConversation => (Operation::SessionBranchCreate, None, true),
        copy::Purpose::Revision { turn_id } => {
            (Operation::SessionRevisionCreate, Some(turn_id), false)
        }
        // The storage API supports a history-end cut; the public wire API does
        // not. Never silently convert that into an empty side conversation.
        copy::Purpose::Branch { turn_id: None, .. } => {
            return Err(ProtocolError::invalid(
                "Session branch requires an explicit Turn boundary",
            ));
        }
    };
    let mut value = json!({
        "sourceSessionId":input.source_session_id,
        "targetSessionId":input.target_session_id,
        "expectedSourceRevision":input.expected_source_revision,
    });
    if let Some(turn) = turn {
        value["sourceTurnId"] = json!(turn);
    }
    if side {
        value["intent"] = json!("side_conversation");
    }
    Ok((operation, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_preserves_history_cut_and_side_intent_without_coercing_end_into_empty() {
        let mut input = copy::Input {
            source_session_id: "source".into(),
            target_session_id: "target".into(),
            expected_source_revision: 7,
            purpose: copy::Purpose::EmptySideConversation,
        };
        for purpose in [
            copy::Purpose::EmptySideConversation,
            copy::Purpose::Branch {
                turn_id: Some("turn".into()),
                side_conversation: false,
            },
            copy::Purpose::Branch {
                turn_id: Some("turn".into()),
                side_conversation: true,
            },
            copy::Purpose::Revision {
                turn_id: "turn".into(),
            },
        ] {
            input.purpose = purpose;
            let (op, value) = wire(&input).unwrap();
            assert_eq!(copy::decode_input(op, &value).unwrap(), input);
        }
        for side_conversation in [false, true] {
            input.purpose = copy::Purpose::Branch {
                turn_id: None,
                side_conversation,
            };
            assert!(wire(&input).is_err());
        }
    }
}
