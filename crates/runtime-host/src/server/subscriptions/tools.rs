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

use super::super::HostError;
use maka_event_log::observation::{StoreStreamEvent, StreamFact, ToolSettlement};
use maka_presentation::tool_message_id;
use maka_protocol::subscription::{SessionToolEvent, ToolResultStatus};
use std::time::UNIX_EPOCH;

/// Live payloads contain activity, not full arguments/results or ancestry. Those
/// are available through the immutable transcript at the same committed fence.
pub(super) fn events(stored: &StoreStreamEvent) -> Result<Vec<SessionToolEvent>, HostError> {
    let (operation, name, status) = match &stored.fact {
        StreamFact::ToolDispatched { operation_id, name } => (operation_id, Some(name), None),
        StreamFact::ToolRejected { operation_id, name } => {
            (operation_id, Some(name), Some(ToolResultStatus::Errored))
        }
        StreamFact::ToolSettled {
            operation_id,
            outcome,
        } => (
            operation_id,
            None,
            Some(match outcome {
                ToolSettlement::Succeeded => ToolResultStatus::Completed,
                ToolSettlement::Failed => ToolResultStatus::Errored,
            }),
        ),
        _ => return Ok(Vec::new()),
    };
    let ts = u64::try_from(stored.recorded_at.duration_since(UNIX_EPOCH)?.as_millis())?;
    let tool_use_id = tool_message_id(&stored.invocation.invocation_id, operation);
    let mut events = Vec::new();
    if let Some(name) = name {
        events.push(SessionToolEvent::ToolStart {
            id: tool_use_id.clone(),
            turn_id: stored.invocation.turn_id.clone(),
            ts,
            tool_use_id: tool_use_id.clone(),
            tool_name: name.clone(),
            // Canonical step:provider IDs are not wire entity IDs. The optional
            // operation ID is omitted, not altered into a different authority.
            operation_id: None,
            step_id: None,
        });
    }
    if let Some(status) = status {
        events.push(SessionToolEvent::ToolResult {
            id: stored.id.clone(),
            turn_id: stored.invocation.turn_id.clone(),
            ts,
            tool_use_id,
            operation_id: None,
            status,
        });
    }
    Ok(events)
}
