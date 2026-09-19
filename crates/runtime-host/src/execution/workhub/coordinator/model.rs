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

use super::*;
use crate::{plugins::workhub::coordinator::model::Prepared, session::SessionTarget};

pub(in crate::execution::workhub) async fn configure(
    executions: &Arc<Executions>,
    caller: Context,
    model: Prepared,
) -> Result<SessionMutation<SessionConfiguration>> {
    let _gate = executions.lock_admission().await;
    let _call = caller
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    if executions.shutdown.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let current = executions
        .workhub_coordinator()
        .await?
        .ok_or_else(|| failure(Code::NotFound, "WorkHub Session has not been resolved"))?;
    if current.revision != model.expected_revision {
        return Ok(SessionMutation::RevisionConflict {
            expected: model.expected_revision,
            actual: current.revision,
        });
    }
    let mut next = current.configuration.clone();
    next.target = SessionTarget::Model { model: model.model };
    next.thinking_level = model.thinking;
    next.connection_locked |= model.lock_connection;
    if next != current.configuration {
        if !executions
            .log
            .pending_interactions(COORDINATION_SESSION_ID)
            .await
            .map_err(|error| stored(executions, error))?
            .is_empty()
        {
            return Err(failure(
                Code::SessionBusy,
                "Session has a pending Interaction",
            ));
        }
        if executions
            .has_session_work(COORDINATION_SESSION_ID)
            .await
            .map_err(|error| stored(executions, error))?
            || current.execution.as_ref().is_some_and(|execution| {
                matches!(execution.state, SessionExecutionState::Live { .. })
            })
        {
            return Err(failure(
                Code::SessionBusy,
                "Session configuration cannot change while a Turn is active",
            ));
        }
    }
    executions
        .log
        .update_session_metadata(
            COORDINATION_SESSION_ID,
            model.expected_revision,
            move |configuration: &mut SessionConfiguration| {
                *configuration = next;
                Ok(())
            },
        )
        .await
        .map_err(|error| stored(executions, error))
}
