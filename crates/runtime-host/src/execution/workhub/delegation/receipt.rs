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

use super::super::commands;
use super::super::{Executions, Result, failure};
use crate::plugins::workhub::delegation::Identity;
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::{
    event::Fact,
    workhub::{COORDINATION_SESSION_ID, Delegation},
};

/// Caller holds admission; an existing canonical action takes precedence over
/// source liveness, target changes and model availability.
pub(super) async fn read(
    executions: &Executions,
    identity: &Identity,
) -> Result<Option<Delegation>> {
    if executions
        .log
        .workhub_stop(&identity.action_id)
        .await
        .map_err(|error| commands::stored(executions, error))?
        .is_some()
        || executions
            .log
            .workhub_correction(&identity.action_id)
            .await
            .map_err(|error| commands::stored(executions, error))?
            .is_some()
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub action already belongs to a control operation",
        ));
    }
    // Receipt authority survives source termination, model removal and target changes.
    if let Some(stored) = executions
        .log
        .workhub_action(&identity.action_id)
        .await
        .map_err(|error| commands::stored(executions, error))?
    {
        let Fact::WorkhubDelegated { delegation } = stored.event.fact else {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub action belongs to another operation",
            ));
        };
        if stored.event.invocation.session_id != COORDINATION_SESSION_ID
            || stored.event.invocation.turn_id != identity.turn_id
            || delegation.request_fingerprint != identity.fingerprint
        {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub action belongs to another request",
            ));
        }
        return Ok(Some(*delegation));
    }
    Ok(None)
}
