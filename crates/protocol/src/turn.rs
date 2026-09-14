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

//! Direct hosted execution, epoch 141. Use decode functions at JSON boundaries.
#[path = "turn_content.rs"]
mod content;
#[path = "turn_resume.rs"]
mod resume;
#[path = "turn_skills.rs"]
mod skills;
#[path = "turn_types.rs"]
mod types;
use crate::{ProtocolError, Result, codec};
pub use content::*;
pub use resume::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
pub use skills::*;
pub use types::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnStartInput {
    pub session_id: String,
    pub turn_id: String,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_orchestration: Option<TurnOrchestration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnQueryInput {
    pub session_id: String,
    pub turn_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnStopInput {
    pub session_id: String,
    pub turn_id: String,
    pub run_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TurnStartResult {
    Started {
        turn: TurnSnapshot,
        skill_invocation: SkillInvocationResult,
    },
    Blocked {
        skill_invocation: SkillInvocationResult,
    },
}

pub fn decode_turn_start_input(value: &Value) -> Result<TurnStartInput> {
    let mut input: TurnStartInput = decode(value)?;
    entity(&input.session_id)?;
    entity(&input.turn_id)?;
    let ids = input.skill_ids.as_deref().unwrap_or_default();
    validate_skill_ids(ids)?;
    input.content.validate_admission(!ids.is_empty())?;
    if ids.is_empty() {
        input.skill_ids = None;
    }
    if let Some(max) = input.max_steps {
        ensure(max > 0, "Invalid maxSteps")?;
    }
    Ok(input)
}

pub(crate) fn validate_skill_ids(ids: &[String]) -> Result<()> {
    ensure(
        ids.len() <= 50
            && ids.iter().all(|id| {
                id.len() <= 512
                    && id.split(':').all(|part| {
                        part.as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphanumeric)
                            && part
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                    })
            }),
        "Invalid skillIds",
    )
}
pub fn decode_turn_query_input(value: &Value) -> Result<TurnQueryInput> {
    let input: TurnQueryInput = decode(value)?;
    entity(&input.session_id)?;
    entity(&input.turn_id)?;
    Ok(input)
}
pub fn decode_turn_stop_input(value: &Value) -> Result<TurnStopInput> {
    let input: TurnStopInput = decode(value)?;
    entity(&input.session_id)?;
    entity(&input.turn_id)?;
    entity(&input.run_id)?;
    Ok(input)
}
pub fn decode_turn_snapshot(value: &Value) -> Result<TurnSnapshot> {
    let snapshot: TurnSnapshot = decode(value)?;
    snapshot.validate(value)?;
    Ok(snapshot)
}
pub fn decode_context_compaction_outcome(value: &Value) -> Result<ContextCompactionOutcome> {
    let outcome: ContextCompactionOutcome = decode(value)?;
    outcome.validate()?;
    Ok(outcome)
}
pub fn decode_turn_start_result(value: &Value) -> Result<TurnStartResult> {
    let result: TurnStartResult = decode(value)?;
    match &result {
        TurnStartResult::Started {
            turn,
            skill_invocation,
        } => {
            turn.validate(&value["turn"])?;
            skill_invocation
                .validate()
                .map_err(ProtocolError::invalid)?;
        }
        TurnStartResult::Blocked { skill_invocation } => {
            skill_invocation
                .validate()
                .map_err(ProtocolError::invalid)?;
            ensure(
                skill_invocation.loaded.is_empty() && !skill_invocation.failed.is_empty(),
                "Blocked Turn requires only failed Skill invocations",
            )?;
        }
    }
    Ok(result)
}
pub fn assert_start_output_for_input(
    input: &TurnStartInput,
    output: &TurnStartResult,
) -> Result<()> {
    if let TurnStartResult::Started { turn, .. } = output {
        ensure(
            input.session_id == turn.session_id && input.turn_id == turn.turn_id,
            "Turn start changed operation identity",
        )?;
    }
    Ok(())
}

// All numeric fields in these contracts are safe nonnegative integers. Normalize
// JSON exponent/decimal spellings before serde's integer deserialization.
fn normalize(value: &Value) -> Result<Value> {
    match value {
        Value::Null => Err(ProtocolError::invalid(
            "Optional fields must be omitted, not null",
        )),
        Value::Number(_) => Ok(Value::from(codec::count(value, "count")?)),
        Value::Array(a) => Ok(Value::Array(
            a.iter().map(normalize).collect::<Result<_>>()?,
        )),
        Value::Object(o) => Ok(Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), normalize(v)?)))
                .collect::<Result<_>>()?,
        )),
        _ => Ok(value.clone()),
    }
}
pub(crate) fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(normalize(value)?).map_err(|e| ProtocolError::invalid(e.to_string()))
}
fn ensure(valid: bool, message: &str) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::invalid(message))
    }
}
pub(crate) fn entity(value: &str) -> Result<()> {
    ensure(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "Invalid entity ID",
    )
}
fn text(value: &str, max: usize) -> Result<()> {
    ensure(
        !value.is_empty() && value.encode_utf16().count() <= max,
        "Invalid string",
    )
}
fn bytes(value: &str, max: usize, empty: bool) -> Result<()> {
    ensure(
        (empty || !value.is_empty()) && value.len() <= max,
        "Invalid UTF-8 string",
    )
}
fn encoded(value: &impl Serialize, max: usize) -> Result<()> {
    let encoded = serde_json::to_vec(value).map_err(|e| ProtocolError::invalid(e.to_string()))?;
    ensure(encoded.len() <= max, "Encoded content exceeds byte limit")
}
