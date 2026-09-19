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

use super::{Host, sessions};
use crate::plugins::workhub::candidates::eligible;
use crate::session::SessionConfiguration;
use maka_event_log::sessions::{SessionExecutionState, SessionRecord};
use maka_protocol::{OperationError, OperationErrorCode as Code};
use std::sync::Arc;

pub(in crate::server) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::PersistenceFailed,
    Code::InternalFailure,
];

pub(super) async fn query(
    host: &Arc<Host>,
) -> Result<crate::plugins::workhub::candidates::Candidates, OperationError> {
    super::control(host)?.value.candidates().await
}

pub(super) async fn target(
    host: &Arc<Host>,
    id: &str,
) -> Result<Option<SessionRecord<SessionConfiguration>>, OperationError> {
    let executions = host.executions.clone();
    host.log
        .workhub_candidate(id, move |record| {
            eligible(&record.id, &record.configuration) && execution_available(&executions, record)
        })
        .await
        .map_err(sessions::stored)
}

fn execution_available(
    executions: &crate::execution::Executions,
    record: &SessionRecord<SessionConfiguration>,
) -> bool {
    if let Some(execution) = &record.execution
        && matches!(execution.state, SessionExecutionState::Live { .. })
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
