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
    ProtocolError, Result,
    codec::{count, exact, record, string},
};
pub use maka_runtime::interaction::*;
use serde::{Serialize, ser::SerializeStruct};
use serde_json::{Value, json};
use std::collections::HashSet;
mod question;
pub use question::project_question_request;

pub const INTERACTION_MAX_PENDING_PER_SESSION: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionQueryInput {
    pub session_id: String,
    pub interaction_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionAnswerInput {
    pub session_id: String,
    pub interaction_id: String,
    pub answer: InteractionAnswer,
}

/// Status, revision and nullability are derived from one canonical outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct InteractionSnapshot {
    record: InteractionRecord,
}

impl InteractionSnapshot {
    pub fn from_record(record: &InteractionRecord) -> Result<Self> {
        record.validate().map_err(ProtocolError::invalid)?;
        let mut record = record.clone();
        // Creation time is not part of the wire snapshot or its equality.
        record.created_at = 0;
        Ok(Self { record })
    }
    pub fn session_id(&self) -> &str {
        &self.record.session_id
    }
    pub fn interaction_id(&self) -> &str {
        &self.record.request_id
    }
    pub fn turn_id(&self) -> &str {
        &self.record.turn_id
    }
    pub fn run_id(&self) -> &str {
        &self.record.run_id
    }
    pub fn request(&self) -> &InteractionRequest {
        &self.record.request
    }
    pub fn outcome(&self) -> Option<&InteractionOutcome> {
        self.record.outcome.as_ref()
    }
    pub fn is_pending(&self) -> bool {
        self.record.outcome.is_none()
    }
    pub fn is_answered(&self) -> bool {
        matches!(
            self.record.outcome,
            Some(
                InteractionOutcome::ClientCapabilityDecision { .. }
                    | InteractionOutcome::FormAnswer { .. }
                    | InteractionOutcome::QuestionAnswer { .. }
            )
        )
    }
}
impl Serialize for InteractionSnapshot {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("InteractionSnapshot", 9)?;
        fields.serialize_field("schemaVersion", &1)?;
        fields.serialize_field("interactionId", self.interaction_id())?;
        fields.serialize_field("sessionId", self.session_id())?;
        fields.serialize_field("turnId", self.turn_id())?;
        fields.serialize_field("runId", self.run_id())?;
        fields.serialize_field("request", self.request())?;
        fields.serialize_field("revision", &if self.is_pending() { 1 } else { 2 })?;
        let status = match self.outcome() {
            None => "pending",
            Some(InteractionOutcome::Closure { .. }) => "closed",
            Some(
                InteractionOutcome::ClientCapabilityDecision { .. }
                | InteractionOutcome::FormAnswer { .. }
                | InteractionOutcome::QuestionAnswer { .. },
            ) => "answered",
        };
        fields.serialize_field("status", status)?;
        fields.serialize_field("outcome", &self.outcome())?;
        fields.end()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SessionInteractionProjection {
    pending: Vec<InteractionSnapshot>,
}
impl SessionInteractionProjection {
    pub fn from_records(records: &[InteractionRecord], session_id: &str) -> Result<Self> {
        let projection = Self {
            pending: records
                .iter()
                .map(InteractionSnapshot::from_record)
                .collect::<Result<_>>()?,
        };
        projection.validate(session_id)?;
        Ok(projection)
    }

    pub fn validate(&self, session_id: &str) -> Result<()> {
        if self.pending.len() > INTERACTION_MAX_PENDING_PER_SESSION {
            return Err(ProtocolError::invalid("Too many pending interactions"));
        }
        let mut ids = HashSet::new();
        for snapshot in &self.pending {
            if !snapshot.is_pending()
                || snapshot.session_id() != session_id
                || !ids.insert(snapshot.interaction_id())
            {
                return Err(ProtocolError::invalid(
                    "Invalid pending interaction identity or state",
                ));
            }
        }
        Ok(())
    }

    pub fn pending(&self) -> &[InteractionSnapshot] {
        &self.pending
    }
}

pub fn decode_request(value: &Value) -> Result<InteractionRequest> {
    let decoded: InteractionRequest = deserialize(value)?;
    decoded.validate().map_err(ProtocolError::invalid)?;
    Ok(decoded)
}
pub fn decode_answer(value: &Value) -> Result<InteractionAnswer> {
    deserialize(value)
}
pub fn decode_outcome(value: &Value) -> Result<InteractionOutcome> {
    let mut normalized = value.clone();
    let timestamp = count(&value["committedAt"], "committedAt")?;
    record(value, "Interaction outcome")?;
    normalized["committedAt"] = json!(timestamp);
    let decoded: InteractionOutcome = deserialize(&normalized)?;
    decoded.validate().map_err(ProtocolError::invalid)?;
    Ok(decoded)
}
pub fn decode_query_input(value: &Value) -> Result<InteractionQueryInput> {
    exact(
        record(value, "Interaction query")?,
        &["sessionId", "interactionId"],
    )?;
    Ok(InteractionQueryInput {
        session_id: identity(&value["sessionId"])?,
        interaction_id: identity(&value["interactionId"])?,
    })
}
pub fn decode_answer_input(value: &Value) -> Result<InteractionAnswerInput> {
    exact(
        record(value, "Interaction answer")?,
        &["sessionId", "interactionId", "answer"],
    )?;
    Ok(InteractionAnswerInput {
        session_id: identity(&value["sessionId"])?,
        interaction_id: identity(&value["interactionId"])?,
        answer: decode_answer(&value["answer"])?,
    })
}
pub fn decode_snapshot(value: &Value) -> Result<InteractionSnapshot> {
    exact(
        record(value, "Interaction snapshot")?,
        &[
            "schemaVersion",
            "interactionId",
            "sessionId",
            "turnId",
            "runId",
            "revision",
            "request",
            "status",
            "outcome",
        ],
    )?;
    if count(&value["schemaVersion"], "schemaVersion")? != 1 {
        return Err(ProtocolError::invalid("Unsupported interaction schema"));
    }
    let revision = count(&value["revision"], "revision")?;
    let outcome = match (value["status"].as_str(), revision) {
        (Some("pending"), 1) if value["outcome"].is_null() => None,
        (Some(status @ ("answered" | "closed")), 2) => {
            let outcome = decode_outcome(&value["outcome"])?;
            if (status == "closed") != matches!(outcome, InteractionOutcome::Closure { .. }) {
                return Err(ProtocolError::invalid(
                    "Interaction status and outcome disagree",
                ));
            }
            Some(outcome)
        }
        _ => {
            return Err(ProtocolError::invalid(
                "Invalid interaction lifecycle state",
            ));
        }
    };
    InteractionSnapshot::from_record(&InteractionRecord {
        session_id: identity(&value["sessionId"])?,
        turn_id: identity(&value["turnId"])?,
        run_id: identity(&value["runId"])?,
        request_id: identity(&value["interactionId"])?,
        created_at: 0,
        request: decode_request(&value["request"])?,
        outcome,
    })
}
pub fn decode_answered_snapshot(value: &Value) -> Result<InteractionSnapshot> {
    let snapshot = decode_snapshot(value)?;
    if !snapshot.is_answered() {
        return Err(ProtocolError::invalid(
            "Interaction answer output is not answered",
        ));
    }
    Ok(snapshot)
}
pub fn decode_session_projection(
    value: &Value,
    session_id: &str,
) -> Result<SessionInteractionProjection> {
    exact(
        record(value, "Session interaction projection")?,
        &["pending"],
    )?;
    let pending = value["pending"]
        .as_array()
        .filter(|values| values.len() <= INTERACTION_MAX_PENDING_PER_SESSION)
        .ok_or_else(|| ProtocolError::invalid("Invalid pending interactions"))?;
    let pending = pending
        .iter()
        .map(decode_snapshot)
        .collect::<Result<Vec<_>>>()?;
    let projection = SessionInteractionProjection { pending };
    projection.validate(session_id)?;
    Ok(projection)
}
fn identity(value: &Value) -> Result<String> {
    let value = string(value, "interaction identity", 128)?;
    entity_id(&value).map_err(ProtocolError::invalid)?;
    Ok(value)
}
fn deserialize<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|_| ProtocolError::invalid("Invalid interaction"))
}
