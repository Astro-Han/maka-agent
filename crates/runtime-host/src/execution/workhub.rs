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

use super::{Executions, Result, failure, internal};
use crate::session::SessionConfiguration;
use maka_event_log::sessions::SessionRecord;
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::workhub::ActionId;
use maka_runtime::{event::Invocation, workhub::COORDINATION_SESSION_ID};
use std::sync::Arc;

mod answer;
mod commands;
mod coordinator;
mod correction;
mod delegation;
mod resume;
mod selection;
mod stop;
pub(crate) use commands::WorkHubCommands;

impl Executions {
    pub(crate) async fn workhub_target(
        self: &Arc<Self>,
        id: &str,
        eligible: crate::plugins::workhub::control::CandidateFilter,
    ) -> Result<Option<SessionRecord<SessionConfiguration>>> {
        let executions = self.clone();
        self.log
            .workhub_candidate(id, move |record| {
                eligible(&record.id, &record.configuration)
                    && execution_available(&executions, record)
            })
            .await
            .map_err(|error| commands::stored(self, error))
    }

    /// Caller owns admission; waiting for cleanup must happen after releasing it.
    pub(crate) async fn stop_workhub_owner(
        &self,
        owner: &Invocation,
        action_id: &ActionId,
    ) -> Result<Option<tokio_util::sync::CancellationToken>> {
        self.retire_owner(
            owner,
            maka_agent::CancellationCause::WorkhubStop {
                action_id: action_id.clone(),
            },
        )
        .await
    }

    pub(crate) async fn workhub_source(
        &self,
        turn: &str,
    ) -> Result<maka_event_log::turns::TurnBoundary> {
        let invocation = self
            .active
            .lock()
            .unwrap()
            .values()
            .find(|run| {
                run.invocation.session_id == COORDINATION_SESSION_ID
                    && run.invocation.turn_id == turn
                    && !run.cancellation.is_cancelled()
            })
            .map(|run| run.invocation.clone())
            .ok_or_else(|| failure(Code::OperationConflict, "WorkHub Turn is not active"))?;
        let boundary = self
            .log
            .run_boundary(COORDINATION_SESSION_ID, &invocation.run_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::OperationConflict, "WorkHub opening is missing"))?;
        if matches!(
            boundary.state,
            maka_event_log::turns::InvocationState::Ended { .. }
        ) {
            return Err(failure(Code::OperationConflict, "WorkHub Turn has ended"));
        }
        Ok(boundary)
    }
}

fn execution_available(
    executions: &Executions,
    record: &SessionRecord<SessionConfiguration>,
) -> bool {
    if let Some(execution) = &record.execution
        && matches!(
            execution.state,
            maka_event_log::sessions::SessionExecutionState::Live { .. }
        )
    {
        // External adapters have no native model-step steering boundary.
        if matches!(
            record.configuration.target,
            crate::session::SessionTarget::Executor { .. }
        ) {
            return false;
        }
        executions
            .active_session_owner(&record.id)
            .is_some_and(|owner| owner.turn_id == execution.turn_id)
    } else {
        !executions.has_active_session(&record.id)
    }
}
