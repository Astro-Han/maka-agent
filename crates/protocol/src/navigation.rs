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

use crate::{Operation, ProtocolError, Result, codec};
pub use maka_presentation::navigation::{RecordedTurnState, TurnContribution, TurnLandmark};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_CONTRIBUTIONS: usize = 128;
pub const MAX_LANDMARKS: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnsInput {
    pub session_id: String,
    pub through_sequence: Option<u64>,
    pub position: u64,
    pub max_contributions: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LandmarksInput {
    pub session_id: String,
    pub max_landmarks: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnsResult {
    pub session_id: String,
    pub through_sequence: Option<u64>,
    pub contributions: Vec<TurnContribution>,
    pub next_position: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LandmarksResult {
    pub session_id: String,
    pub through_sequence: Option<u64>,
    pub landmarks: Vec<TurnLandmark>,
}

pub fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::SessionTurnsQuery | Operation::SessionTurnLandmarksQuery
    )
}
pub fn decode_turns_input(value: &Value) -> Result<TurnsInput> {
    codec::exact(
        codec::record(value, "Turn query")?,
        &[
            "sessionId",
            "throughSequence",
            "position",
            "maxContributions",
        ],
    )?;
    Ok(TurnsInput {
        session_id: entity(&value["sessionId"])?,
        through_sequence: nullable_count(&value["throughSequence"])?,
        position: codec::count(&value["position"], "position")?,
        max_contributions: limit(&value["maxContributions"], MAX_CONTRIBUTIONS)?,
    })
}
pub fn decode_landmarks_input(value: &Value) -> Result<LandmarksInput> {
    codec::exact(
        codec::record(value, "Turn landmarks query")?,
        &["sessionId", "maxLandmarks"],
    )?;
    Ok(LandmarksInput {
        session_id: entity(&value["sessionId"])?,
        max_landmarks: limit(&value["maxLandmarks"], MAX_LANDMARKS)?,
    })
}
pub fn decode_input(operation: Operation, value: &Value) -> Result<Value> {
    let value = match operation {
        Operation::SessionTurnsQuery => serde_json::to_value(decode_turns_input(value)?),
        Operation::SessionTurnLandmarksQuery => {
            serde_json::to_value(decode_landmarks_input(value)?)
        }
        _ => return Err(invalid()),
    };
    value.map_err(|_| invalid())
}
pub fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::SessionTurnsQuery => {
            codec::exact(
                codec::record(value, "Turn query result")?,
                &[
                    "sessionId",
                    "throughSequence",
                    "contributions",
                    "nextPosition",
                ],
            )?;
            envelope(value, 192 * 1024)?;
            let mut normalized = value.clone();
            normalized["throughSequence"] =
                serde_json::json!(nullable_count(&value["throughSequence"])?);
            normalized["nextPosition"] = serde_json::json!(nullable_count(&value["nextPosition"])?);
            let rows = normalized["contributions"]
                .as_array_mut()
                .filter(|a| a.len() <= MAX_CONTRIBUTIONS)
                .ok_or_else(invalid)?;
            for row in rows {
                codec::exact(
                    codec::record(row, "Turn contribution")?,
                    &[
                        "turnId",
                        "firstSequence",
                        "latestState",
                        "userPromptPreview",
                    ],
                )?;
                entity(&row["turnId"])?;
                row["firstSequence"] =
                    serde_json::json!(codec::count(&row["firstSequence"], "firstSequence")?);
                if !row["userPromptPreview"].is_null() {
                    utf8(&row["userPromptPreview"], 256)?;
                }
                if !row["latestState"].is_null() {
                    let state = &mut row["latestState"];
                    codec::exact(
                        codec::record(state, "recorded Turn state")?,
                        &["sequence", "message"],
                    )?;
                    state["sequence"] =
                        serde_json::json!(codec::count(&state["sequence"], "sequence")?);
                    normalize_state(&mut state["message"])?;
                }
            }
            let _: TurnsResult =
                serde_json::from_value(normalized.clone()).map_err(|_| invalid())?;
            Ok(normalized)
        }
        Operation::SessionTurnLandmarksQuery => {
            codec::exact(
                codec::record(value, "Turn landmarks result")?,
                &["sessionId", "throughSequence", "landmarks"],
            )?;
            envelope(value, 64 * 1024)?;
            let mut normalized = value.clone();
            normalized["throughSequence"] =
                serde_json::json!(nullable_count(&value["throughSequence"])?);
            let rows = normalized["landmarks"]
                .as_array_mut()
                .filter(|a| a.len() <= MAX_LANDMARKS)
                .ok_or_else(invalid)?;
            for row in rows {
                codec::exact(
                    codec::record(row, "Turn landmark")?,
                    &["turnId", "sequence", "label"],
                )?;
                entity(&row["turnId"])?;
                row["sequence"] = serde_json::json!(codec::count(&row["sequence"], "sequence")?);
                utf8(&row["label"], 96)?;
            }
            let _: LandmarksResult =
                serde_json::from_value(normalized.clone()).map_err(|_| invalid())?;
            Ok(normalized)
        }
        _ => Err(invalid()),
    }
}
fn normalize_state(value: &mut Value) -> Result<()> {
    let frame = codec::record(value, "Turn state message")?;
    codec::shaped(
        frame,
        &["type", "id", "turnId", "ts", "status"],
        &[
            "parentTurnId",
            "retriedFromTurnId",
            "regeneratedFromTurnId",
            "branchOfTurnId",
            "parentSessionId",
            "abortedAt",
            "abortSource",
            "errorClass",
            "failureMessage",
            "retry",
            "partialOutputRetained",
        ],
    )?;
    if value["type"] != "turn_state"
        || !value["id"].is_string()
        || !value["turnId"].is_string()
        || !value["ts"].as_f64().is_some_and(f64::is_finite)
    {
        return Err(invalid());
    }
    for name in [
        "parentTurnId",
        "retriedFromTurnId",
        "regeneratedFromTurnId",
        "branchOfTurnId",
        "parentSessionId",
        "abortSource",
        "errorClass",
        "failureMessage",
    ] {
        if frame.get(name).is_some_and(|v| !v.is_string()) {
            return Err(invalid());
        }
    }
    if frame
        .get("abortedAt")
        .is_some_and(|v| !v.as_f64().is_some_and(f64::is_finite))
    {
        return Err(invalid());
    }
    for (name, bytes) in [
        ("abortSource", 128),
        ("errorClass", 128),
        ("failureMessage", 2048),
    ] {
        if let Some(text) = frame.get(name) {
            utf8(text, bytes)?;
        }
    }
    if let Some(retry) = value.get_mut("retry") {
        if retry["decision"] == "exhausted" {
            let attempts = codec::count(&retry["attempts"], "retry attempts")?;
            if attempts == 0 {
                return Err(invalid());
            }
            retry["attempts"] = serde_json::json!(attempts);
        } else if retry.is_null() {
            return Err(invalid());
        }
    }
    value
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove("partialOutputRetained");
    Ok(())
}
fn envelope(value: &Value, bytes: usize) -> Result<()> {
    entity(&value["sessionId"])?;
    maka_runtime::capability::json::encoded_limit(value, bytes).map_err(|_| invalid())?;
    Ok(())
}
fn utf8(value: &Value, bytes: usize) -> Result<()> {
    value
        .as_str()
        .filter(|s| s.len() <= bytes)
        .map(|_| ())
        .ok_or_else(invalid)
}
fn nullable_count(value: &Value) -> Result<Option<u64>> {
    if value.is_null() {
        Ok(None)
    } else {
        codec::count(value, "sequence").map(Some)
    }
}
fn limit(value: &Value, max: usize) -> Result<usize> {
    let count = codec::count(value, "navigation limit")?;
    if count == 0 || count > max as u64 {
        return Err(invalid());
    }
    Ok(count as usize)
}
fn entity(value: &Value) -> Result<String> {
    let id = codec::string(value, "entity ID", 128)?;
    crate::turn::entity(&id)?;
    Ok(id)
}
fn invalid() -> ProtocolError {
    ProtocolError::invalid("Invalid Session Turn navigation")
}
