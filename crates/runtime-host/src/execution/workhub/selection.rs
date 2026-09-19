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

use super::{
    super::{Executions, Result, failure},
    commands::stored,
};
use crate::plugins::workhub::selection::{Source, request_id};
use maka_plugins::fiber::Context;
use maka_protocol::{OperationErrorCode as Code, workhub::SelectionInput};
use maka_runtime::{
    event::Invocation,
    interaction::{InteractionRecord, InteractionRequest},
    workhub::COORDINATION_SESSION_ID,
};
use std::sync::Arc;

pub(super) async fn inspect(
    executions: &Arc<Executions>,
    caller: Context,
    input: SelectionInput,
) -> Result<Source> {
    let _gate = executions.lock_admission().await;
    let _call = caller
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    source(executions, &input).await
}

pub(super) async fn offer(
    executions: &Arc<Executions>,
    caller: Context,
    input: SelectionInput,
    invocation: Invocation,
    request: InteractionRequest,
) -> Result<InteractionRecord> {
    let _gate = executions.lock_admission().await;
    let _call = caller
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    let source = source(executions, &input).await?;
    if source.invocation != invocation {
        return Err(failure(
            Code::OperationConflict,
            "The selecting Run is no longer active",
        ));
    }
    if let Some(form) = source.form {
        return Ok(form);
    }
    if !matches!(&request, InteractionRequest::Form { tool_use_id, .. }
        if tool_use_id == input.action_id.as_str())
    {
        return Err(failure(
            Code::OperationConflict,
            "Target choice belongs to another request",
        ));
    }
    executions
        .interactions
        .admit_stable_request(
            invocation,
            source.request_id,
            request,
            &caller
                .stopping()
                .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?,
        )
        .await
        .map_err(crate::plugins::workhub::target::context_error)
}

/// Called under admission. Existing offers retain their original source and
/// outcome even after the source ended; only a new offer needs a live owner.
async fn source(executions: &Executions, input: &SelectionInput) -> Result<Source> {
    let committed = executions
        .log
        .workhub_action(&input.action_id)
        .await
        .map_err(|error| stored(executions, error))?;
    let invocation = match &committed {
        Some(action) if action.event.invocation.turn_id == input.turn_id => {
            action.event.invocation.clone()
        }
        Some(_) => {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub action belongs to another Turn",
            ));
        }
        None => {
            executions
                .log
                .turn_boundary(COORDINATION_SESSION_ID, &input.turn_id)
                .await
                .map_err(|error| stored(executions, error))?
                .ok_or_else(|| failure(Code::OperationConflict, "WorkHub Turn does not exist"))?
                .invocation
        }
    };
    let request_id = request_id(input, &invocation)?;
    let form = executions
        .log
        .interaction(&request_id)
        .await
        .map_err(|error| stored(executions, error))?;
    if let Some(form) = &form {
        if form.session_id != invocation.session_id
            || form.turn_id != invocation.turn_id
            || form.run_id != invocation.run_id
            || !matches!(&form.request, InteractionRequest::Form { tool_use_id, .. }
                if tool_use_id == input.action_id.as_str())
        {
            return Err(failure(
                Code::OperationConflict,
                "Target choice belongs to another request",
            ));
        }
    } else {
        if committed.is_some() {
            return Err(failure(
                Code::OperationConflict,
                "This action identity already belongs to another request",
            ));
        }
        if executions.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        if executions.workhub_source(&input.turn_id).await?.invocation != invocation {
            return Err(failure(
                Code::OperationConflict,
                "The selecting Run is no longer active",
            ));
        }
    }
    Ok(Source {
        invocation,
        request_id,
        form,
    })
}
