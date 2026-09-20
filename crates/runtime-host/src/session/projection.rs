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

use crate::session::{SessionConfiguration, metadata_projection};
use maka_event_log::sessions::{SessionExecutionState, SessionRecord};
use maka_protocol::session::*;
use maka_runtime::event::TerminalStatus;

pub(crate) fn mutation_projection(
    mutation: maka_event_log::sessions::SessionMutation<SessionConfiguration>,
) -> SessionUpdateResult {
    use maka_event_log::sessions::SessionMutation;
    match mutation {
        SessionMutation::Committed(record) => SessionUpdateResult::Committed {
            session: Box::new(catalog_projection(record)),
        },
        SessionMutation::RevisionConflict { expected, actual } => {
            SessionUpdateResult::RevisionConflict {
                expected_revision: expected,
                actual_revision: actual,
            }
        }
    }
}

pub fn catalog_projection(
    mut record: SessionRecord<SessionConfiguration>,
) -> SessionCatalogProjection {
    let execution = record.execution.take();
    let pending_since = record.pending_interaction_since;
    let mut projection = metadata_projection(record);
    projection.activity_at = projection.created_at;
    if let Some(since) = pending_since {
        projection.status = SessionStatus::WaitingForUser;
        projection.status_updated_at = Some(since);
    }
    let Some(execution) = execution else {
        return projection;
    };
    let mut running_turn_ids = Vec::new();
    let (status, recorded_at) = match execution.state {
        SessionExecutionState::Live { recorded_at }
        | SessionExecutionState::Ended {
            status: TerminalStatus::Paused,
            recorded_at,
        } => {
            running_turn_ids.push(execution.turn_id);
            (SessionStatus::Running, recorded_at)
        }
        SessionExecutionState::Ended {
            status: TerminalStatus::Completed,
            recorded_at,
        } => (SessionStatus::Active, recorded_at),
        SessionExecutionState::Ended {
            status: TerminalStatus::Cancelled,
            recorded_at,
        } => (SessionStatus::Aborted, recorded_at),
        SessionExecutionState::Ended {
            status: TerminalStatus::Failed,
            recorded_at,
        } => {
            projection.blocked_reason = Some(BlockedReason::Unknown);
            (SessionStatus::Blocked, recorded_at)
        }
    };
    if pending_since.is_none() {
        projection.status = status;
        projection.status_updated_at = Some(recorded_at);
    }
    projection.live_run_state = Some(SessionCatalogLiveRunState {
        schema_version: 1,
        running_turn_ids,
    });
    if let Some(message) = execution.last_message {
        projection.activity_at = message.recorded_at;
        projection.last_message_at = Some(message.recorded_at);
        projection.last_message_preview = message.preview;
    }
    projection
}
