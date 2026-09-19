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

use super::{Host, action, failure, record, sessions};
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    workhub::{SelectionInput, SelectionResult},
};
use maka_runtime::{
    artifact::content_digest, interaction::InteractionRequest, workhub::COORDINATION_SESSION_ID,
};
use std::sync::Arc;

pub(super) use crate::plugins::workhub::selection::{SelectedTarget, workspace_digest};

pub(super) async fn select(
    host: &Arc<Host>,
    input: SelectionInput,
) -> Result<SelectionResult, OperationError> {
    let (invocation, form) = {
        let _gate = host.executions.lock_admission().await;
        let committed = host
            .log
            .workhub_action(&input.action_id)
            .await
            .map_err(sessions::stored)?;
        let invocation = match &committed {
            Some(action) if action.event.invocation.turn_id == input.turn_id => {
                action.event.invocation.clone()
            }
            Some(_) => return Err(conflict("WorkHub action belongs to another Turn")),
            None => {
                host.log
                    .turn_boundary(COORDINATION_SESSION_ID, &input.turn_id)
                    .await
                    .map_err(sessions::stored)?
                    .ok_or_else(|| conflict("WorkHub Turn does not exist"))?
                    .invocation
            }
        };
        let request_id = format!(
            "whf_{}",
            &content_digest(
                &serde_json::to_vec(&("workhub.selection.v1", &input, &invocation),)
                    .map_err(|error| failure(Code::InternalFailure, error.to_string()))?
            )[7..]
        );
        let form = match host
            .log
            .interaction(&request_id)
            .await
            .map_err(sessions::stored)?
        {
            Some(form) => form,
            None => {
                if committed.is_some() {
                    return Err(conflict(
                        "This action identity already belongs to another request",
                    ));
                }
                if host.draining.is_cancelled() {
                    return Err(failure(Code::HostDraining, "Host is draining"));
                }
                record(host)
                    .await?
                    .ok_or_else(|| conflict("WorkHub Session has not been resolved"))?;
                if host
                    .executions
                    .workhub_source(&input.turn_id)
                    .await?
                    .invocation
                    != invocation
                {
                    return Err(conflict("The selecting Run is no longer active"));
                }
                let page = super::candidates::query(host).await?.result;
                let request = crate::plugins::workhub::selection::offer::build(&page, &input)?;
                host.interactions
                    .admit_stable_request(invocation.clone(), request_id, request, &host.draining)
                    .await
                    .map_err(|mut error| {
                        if error.code == Code::InvalidRequest {
                            error.code = Code::OperationConflict;
                        }
                        error
                    })?
            }
        };
        if form.session_id != invocation.session_id
            || form.turn_id != invocation.turn_id
            || form.run_id != invocation.run_id
            || !matches!(&form.request, InteractionRequest::Form { tool_use_id, .. } if tool_use_id == input.action_id.as_str())
        {
            return Err(conflict("Target choice belongs to another request"));
        }
        (invocation, form)
    };
    let outcome = host
        .interactions
        .wait_for_outcome(&form.request_id, &host.draining)
        .await?;
    let Some(selection) =
        crate::plugins::workhub::selection::interpret(input, invocation, form.created_at, outcome)?
    else {
        return Ok(SelectionResult::Cancelled);
    };
    let result = action::selected(host, selection.input, &selection.target).await?;
    Ok(SelectionResult::Delegated { result })
}

fn conflict(reason: &str) -> OperationError {
    failure(Code::OperationConflict, reason)
}
