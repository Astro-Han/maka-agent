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

use super::{TurnSnapshot, decode, ensure, entity};
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnResumeQueryInput {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_runtime_event_high_water: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnResumeStartInput {
    pub session_id: String,
    pub turn_id: String,
    pub source_run_id: String,
    pub source_runtime_event_high_water: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnResumeParkReason {
    ResumeCandidateMissing,
    SourceRunUnreadable,
    SafetyCheckFailed,
    ContinuationAlreadyExists,
    ContinuationRepairRequired,
    ContinuationStartedIndeterminate,
    ResumeFeatureDisabled,
    ContinuationAuthorityUnavailable,
    SafetyObservationUnavailable,
    SessionBusy,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "disposition",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TurnResumePlan {
    Ready {
        session_id: String,
        source_run_id: String,
        source_turn_id: String,
        source_runtime_event_high_water: u64,
    },
    Parked {
        session_id: String,
        reason: TurnResumeParkReason,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TurnResumeStartResult {
    Started { turn: TurnSnapshot },
    Parked { plan: TurnResumePlan },
}

pub fn decode_turn_resume_query_input(value: &Value) -> Result<TurnResumeQueryInput> {
    let input: TurnResumeQueryInput = decode(value)?;
    entity(&input.session_id)?;
    if let Some(id) = &input.source_run_id {
        entity(id)?;
    }
    if let Some(high_water) = input.expected_runtime_event_high_water {
        ensure(
            input.source_run_id.is_some() && high_water > 0,
            "Resume high-water requires a source Run",
        )?;
    }
    Ok(input)
}
pub fn decode_turn_resume_start_input(value: &Value) -> Result<TurnResumeStartInput> {
    let input: TurnResumeStartInput = decode(value)?;
    entity(&input.session_id)?;
    entity(&input.turn_id)?;
    entity(&input.source_run_id)?;
    ensure(
        input.source_runtime_event_high_water > 0,
        "Invalid source high-water",
    )?;
    Ok(input)
}
pub fn decode_turn_resume_plan(value: &Value) -> Result<TurnResumePlan> {
    let plan: TurnResumePlan = decode(value)?;
    match &plan {
        TurnResumePlan::Ready {
            session_id,
            source_run_id,
            source_turn_id,
            source_runtime_event_high_water,
        } => {
            entity(session_id)?;
            entity(source_run_id)?;
            entity(source_turn_id)?;
            ensure(
                *source_runtime_event_high_water > 0,
                "Invalid source high-water",
            )?;
        }
        TurnResumePlan::Parked { session_id, .. } => entity(session_id)?,
    }
    Ok(plan)
}
pub fn decode_turn_resume_start_result(value: &Value) -> Result<TurnResumeStartResult> {
    let result: TurnResumeStartResult = decode(value)?;
    match &result {
        TurnResumeStartResult::Started { turn } => turn.validate(&value["turn"])?,
        TurnResumeStartResult::Parked { plan } => {
            decode_turn_resume_plan(&value["plan"])?;
            ensure(
                matches!(plan, TurnResumePlan::Parked { .. }),
                "Parked result requires a parked plan",
            )?;
        }
    }
    Ok(result)
}
pub fn assert_resume_query_output_for_input(
    input: &TurnResumeQueryInput,
    output: &TurnResumePlan,
) -> Result<()> {
    match output {
        TurnResumePlan::Ready {
            session_id,
            source_run_id,
            source_runtime_event_high_water,
            ..
        } => ensure(
            session_id == &input.session_id
                && input
                    .source_run_id
                    .as_ref()
                    .is_none_or(|id| id == source_run_id)
                && input
                    .expected_runtime_event_high_water
                    .is_none_or(|high_water| high_water == *source_runtime_event_high_water),
            "Resume query changed source identity",
        ),
        TurnResumePlan::Parked { session_id, .. } => ensure(
            session_id == &input.session_id,
            "Resume query changed Session",
        ),
    }
}
pub fn assert_resume_start_output_for_input(
    input: &TurnResumeStartInput,
    output: &TurnResumeStartResult,
) -> Result<()> {
    match output {
        TurnResumeStartResult::Started { turn } => ensure(
            turn.session_id == input.session_id && turn.turn_id == input.turn_id,
            "Resume start changed Turn identity",
        ),
        TurnResumeStartResult::Parked {
            plan: TurnResumePlan::Parked { session_id, .. },
        } => ensure(
            session_id == &input.session_id,
            "Resume start changed Session",
        ),
        TurnResumeStartResult::Parked { .. } => {
            ensure(false, "Parked result requires a parked plan")
        }
    }
}
