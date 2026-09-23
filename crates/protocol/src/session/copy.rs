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

use super::{SessionCatalogProjection, validation};
use crate::{Operation, ProtocolError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use maka_runtime::session::{CopyPurpose as Purpose, CopyRequest as Input, CopyState as State};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Output {
    Committed {
        session: Box<SessionCatalogProjection>,
    },
    SourceRevisionConflict {
        expected_revision: u64,
        actual_revision: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AbandonInput {
    pub target_session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryInput {
    pub target_session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryResult {
    pub receipt: Option<maka_runtime::session::CopyReceipt>,
}

pub fn decode_query_input(value: &Value) -> Result<QueryInput> {
    let input: QueryInput = validation::decode(value)?;
    validation::entity(&input.target_session_id)?;
    Ok(input)
}

pub fn decode_query_result(value: &Value) -> Result<QueryResult> {
    let mut value = value.clone();
    crate::codec::exact(
        crate::codec::record(&value, "Session copy query")?,
        &["receipt"],
    )?;
    if !value["receipt"].is_null() {
        let purpose = &value["receipt"]["request"]["purpose"];
        if purpose["kind"] == "branch" {
            crate::codec::exact(
                crate::codec::record(purpose, "Branch purpose")?,
                &["kind", "turnId", "sideConversation"],
            )?;
        }
        let revision = crate::codec::count(
            &value["receipt"]["request"]["expectedSourceRevision"],
            "expectedSourceRevision",
        )?;
        value["receipt"]["request"]["expectedSourceRevision"] = revision.into();
    }
    let output: QueryResult =
        serde_json::from_value(value).map_err(|error| ProtocolError::invalid(error.to_string()))?;
    if let Some(receipt) = &output.receipt {
        let request = &receipt.request;
        validation::entity(&request.source_session_id)?;
        validation::entity(&request.target_session_id)?;
        if request.source_session_id == request.target_session_id
            || request.expected_source_revision == 0
        {
            return Err(ProtocolError::invalid("Invalid copy receipt identity"));
        }
        match &request.purpose {
            Purpose::Branch {
                turn_id: Some(turn),
                ..
            }
            | Purpose::Revision { turn_id: turn } => validation::entity(turn)?,
            _ => {}
        }
        if receipt.state != maka_runtime::session::CopyState::Committed
            && !matches!(request.purpose, Purpose::Revision { .. })
        {
            return Err(ProtocolError::invalid(
                "Only revisions have a draft lifecycle",
            ));
        }
    }
    Ok(output)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AbandonOutput {
    Abandoned { session_id: String },
    Retained { session_id: String },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireInput {
    source_session_id: String,
    target_session_id: String,
    expected_source_revision: u64,
    source_turn_id: Option<String>,
    intent: Option<Intent>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Intent {
    SideConversation,
}

pub fn decode_input(operation: Operation, value: &Value) -> Result<Input> {
    let input: WireInput = validation::decode(value)?;
    validation::entity(&input.source_session_id)?;
    validation::entity(&input.target_session_id)?;
    if input.source_session_id == input.target_session_id || input.expected_source_revision == 0 {
        return Err(ProtocolError::invalid(
            "Copy requires distinct Sessions and a positive revision",
        ));
    }
    if let Some(turn) = &input.source_turn_id {
        validation::entity(turn)?;
    }
    let purpose = match (operation, input.source_turn_id, input.intent) {
        (Operation::SessionBranchCreate, Some(turn_id), intent) => Purpose::Branch {
            turn_id: Some(turn_id),
            side_conversation: intent.is_some(),
        },
        (Operation::SessionBranchCreate, None, Some(Intent::SideConversation)) => {
            Purpose::EmptySideConversation
        }
        (Operation::SessionRevisionCreate, Some(turn_id), None) => Purpose::Revision { turn_id },
        _ => {
            return Err(ProtocolError::invalid(
                "Invalid Session copy boundary or intent",
            ));
        }
    };
    Ok(Input {
        source_session_id: input.source_session_id,
        target_session_id: input.target_session_id,
        expected_source_revision: input.expected_source_revision,
        purpose,
    })
}

pub fn decode_output(input: &Input, value: &Value) -> Result<Output> {
    let output = decode_result(value)?;
    let matches = match &output {
        Output::Committed { session } => session.id == input.target_session_id,
        Output::SourceRevisionConflict {
            expected_revision, ..
        } => *expected_revision == input.expected_source_revision,
    };
    if !matches {
        return Err(ProtocolError::invalid(
            "Session copy receipt does not match its request",
        ));
    }
    Ok(output)
}

pub fn decode_result(value: &Value) -> Result<Output> {
    let output: Output = validation::decode(value)?;
    match &output {
        Output::Committed { .. } => {
            super::decode_session_catalog_projection(&value["session"])?;
        }
        Output::SourceRevisionConflict {
            expected_revision,
            actual_revision,
        } => {
            if *expected_revision == 0 || *actual_revision == 0 {
                return Err(ProtocolError::invalid("Invalid Session copy revisions"));
            }
        }
    }
    Ok(output)
}

pub fn decode_abandon_input(value: &Value) -> Result<AbandonInput> {
    let input: AbandonInput = validation::decode(value)?;
    validation::entity(&input.target_session_id)?;
    Ok(input)
}

pub fn decode_abandon_output(input: &AbandonInput, value: &Value) -> Result<AbandonOutput> {
    let output = decode_abandon_result(value)?;
    let (AbandonOutput::Abandoned { session_id } | AbandonOutput::Retained { session_id }) =
        &output;
    if session_id != &input.target_session_id {
        return Err(ProtocolError::invalid(
            "Abandon receipt does not match its request",
        ));
    }
    Ok(output)
}

pub fn decode_abandon_result(value: &Value) -> Result<AbandonOutput> {
    let output: AbandonOutput = validation::decode(value)?;
    let (AbandonOutput::Abandoned { session_id } | AbandonOutput::Retained { session_id }) =
        &output;
    validation::entity(session_id)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn copy_intents_keep_exact_boundaries_and_receipt_identity() {
        let request = json!({
            "sourceSessionId": "source", "targetSessionId": "target",
            "expectedSourceRevision": 2.0, "sourceTurnId": "turn",
        });
        let revision = decode_input(Operation::SessionRevisionCreate, &request).unwrap();
        assert_eq!(
            revision.purpose,
            Purpose::Revision {
                turn_id: "turn".into()
            }
        );
        assert!(
            decode_output(
                &revision,
                &json!({
                    "kind": "source_revision_conflict", "expectedRevision": 3, "actualRevision": 4,
                })
            )
            .is_err()
        );
        let mut side = request.clone();
        side["intent"] = json!("side_conversation");
        assert!(decode_input(Operation::SessionRevisionCreate, &side).is_err());
        assert_eq!(
            decode_input(Operation::SessionBranchCreate, &side)
                .unwrap()
                .purpose,
            Purpose::Branch {
                turn_id: Some("turn".into()),
                side_conversation: true
            }
        );
        side.as_object_mut().unwrap().remove("sourceTurnId");
        assert_eq!(
            decode_input(Operation::SessionBranchCreate, &side)
                .unwrap()
                .purpose,
            Purpose::EmptySideConversation
        );
        side.as_object_mut().unwrap().remove("intent");
        assert!(decode_input(Operation::SessionBranchCreate, &side).is_err());
        for (field, value) in [
            ("sourceTurnId", Value::Null),
            ("expectedSourceRevision", json!(0)),
            ("expectedSourceRevision", json!(2.5)),
            ("expectedSourceRevision", json!(9_007_199_254_740_992u64)),
            ("targetSessionId", json!("source")),
        ] {
            let mut invalid = request.clone();
            invalid[field] = value;
            assert!(decode_input(Operation::SessionBranchCreate, &invalid).is_err());
        }
        let abandon = decode_abandon_input(&json!({"targetSessionId": "target"})).unwrap();
        assert!(
            decode_abandon_output(&abandon, &json!({"kind":"retained", "sessionId":"another"}))
                .is_err()
        );
        let receipt = json!({"receipt": {
            "request": {
                "sourceSessionId":"source", "targetSessionId":"target",
                "expectedSourceRevision":2.0, "purpose":{"kind":"revision", "turnId":"turn"}
            },
            "state":"abandoned"
        }});
        assert_eq!(
            decode_query_result(&receipt)
                .unwrap()
                .receipt
                .unwrap()
                .request,
            revision
        );
        assert!(
            decode_query_result(&json!({"receipt":null}))
                .unwrap()
                .receipt
                .is_none()
        );
        let mut branch = receipt;
        branch["receipt"]["request"]["purpose"] =
            json!({"kind":"branch", "turnId":null, "sideConversation":false});
        assert!(decode_query_result(&branch).is_err());
        branch["receipt"]["state"] = json!("committed");
        assert!(decode_query_result(&branch).is_ok());
        branch["receipt"]["request"]["purpose"]
            .as_object_mut()
            .unwrap()
            .remove("turnId");
        assert!(decode_query_result(&branch).is_err());
    }
}
