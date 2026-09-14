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

//! Minimal outbound epoch-141 tool observation subset, not a full inbound
//! SessionToolEvent decoder. Only start/result with operation/step identity are
//! supported. Rich presentation, previews, progress and output are unimplemented.
//! Live events never carry full args/results, ancestry, origin or model visibility.
use super::{SUBSCRIPTION_FRAME_MAX_BYTES, decode, ensure, entity, id};
use crate::{ProtocolError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SESSION_TOOL_NAME_MAX_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    Completed,
    Errored,
}

/// Callers must use the decode functions for semantic validation of JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SessionToolEvent {
    ToolStart {
        id: String,
        turn_id: String,
        ts: u64,
        tool_use_id: String,
        tool_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        step_id: Option<String>,
    },
    ToolResult {
        id: String,
        turn_id: String,
        ts: u64,
        tool_use_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
        status: ToolResultStatus,
    },
}

/// Session event frames carrying only the supported outbound tool subset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum ToolObservationFrame {
    #[serde(rename = "subscription.session_event")]
    SessionEvent {
        host_epoch: String,
        subscription_id: String,
        sequence: u64,
        session_id: String,
        run_id: String,
        event: SessionToolEvent,
    },
}

pub fn decode_session_tool_event(value: &Value) -> Result<SessionToolEvent> {
    let event = decode(value)?;
    validate_event(&event)?;
    Ok(event)
}

pub fn decode_tool_observation_frame(value: &Value) -> Result<ToolObservationFrame> {
    ensure(
        serde_json::to_vec(value)
            .map_err(|e| ProtocolError::invalid(e.to_string()))?
            .len()
            <= SUBSCRIPTION_FRAME_MAX_BYTES,
        "Subscription frame exceeds byte limit",
    )?;
    let frame = decode(value)?;
    let ToolObservationFrame::SessionEvent {
        host_epoch,
        subscription_id,
        sequence,
        session_id,
        run_id,
        event,
    } = &frame;
    id(host_epoch)?;
    id(subscription_id)?;
    ensure(*sequence > 0, "Invalid subscription sequence")?;
    entity(session_id)?;
    entity(run_id)?;
    validate_event(event)?;
    Ok(frame)
}

fn validate_event(event: &SessionToolEvent) -> Result<()> {
    let (event_id, turn_id, tool_use_id, operation_id) = match event {
        SessionToolEvent::ToolStart {
            id,
            turn_id,
            tool_use_id,
            tool_name,
            operation_id,
            step_id,
            ..
        } => {
            ensure(
                !tool_name.is_empty() && tool_name.len() <= SESSION_TOOL_NAME_MAX_BYTES,
                "Invalid Session tool name",
            )?;
            if let Some(step_id) = step_id {
                entity(step_id)?;
            }
            (id, turn_id, tool_use_id, operation_id)
        }
        SessionToolEvent::ToolResult {
            id,
            turn_id,
            tool_use_id,
            operation_id,
            ..
        } => (id, turn_id, tool_use_id, operation_id),
    };
    id(event_id)?;
    entity(turn_id)?;
    id(tool_use_id)?;
    if let Some(operation_id) = operation_id {
        entity(operation_id)?;
    }
    Ok(())
}
