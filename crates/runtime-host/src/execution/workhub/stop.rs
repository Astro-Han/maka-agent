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
use crate::plugins::workhub::control::Stop;
use maka_event_log::{
    StoreError,
    workhub::stop::{StopRecord, StopRequest},
};
use maka_plugins::fiber::Context;
use maka_protocol::OperationErrorCode as Code;
use std::sync::Arc;

pub(super) async fn execute(
    executions: &Arc<Executions>,
    caller: Context,
    request: Stop,
) -> Result<StopRecord> {
    let (record, completed, _call) = {
        let _gate = executions.lock_admission().await;
        let call = caller
            .admit()
            .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
        let record = if let Some(record) = executions
            .log
            .workhub_stop(&request.action_id)
            .await
            .map_err(|error| stored(executions, error))?
        {
            if record.intent.request.source.turn_id != request.turn_id
                || record.intent.request.request_fingerprint != request.request_fingerprint
                || record.intent.request.target_session_id != request.target_session_id
            {
                return Err(failure(
                    Code::OperationConflict,
                    "WorkHub stop belongs to another request",
                ));
            }
            record
        } else {
            if executions.shutdown.is_cancelled() {
                return Err(failure(Code::HostDraining, "Host is draining"));
            }
            let source = executions.workhub_source(&request.turn_id).await?;
            executions
                .log
                .request_workhub_stop(StopRequest {
                    action_id: request.action_id,
                    request_fingerprint: request.request_fingerprint,
                    source: source.invocation,
                    target_session_id: request.target_session_id,
                })
                .await
                .map_err(|error| stored(executions, error))?
        };
        if record.resolution.is_some() {
            return Ok(record);
        }
        if executions.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let completed =
            executions
                .stop_workhub_owner(
                    record.intent.owner.as_ref().ok_or_else(|| {
                        failure(Code::InternalFailure, "Stop intent has no owner")
                    })?,
                    &record.intent.request.action_id,
                )
                .await?;
        (record, completed, call)
    };
    if let Some(completed) = completed {
        completed.cancelled().await;
    }
    let resolved = executions
        .log
        .resolve_workhub_stop(&record.intent.request.action_id)
        .await
        .map_err(|error| {
            if matches!(error, StoreError::SessionBusy) {
                failure(
                    Code::OperationUnavailable,
                    "WorkHub stop owner is still recovering",
                )
            } else {
                stored(executions, error)
            }
        })?;
    let mut admission = Some(executions.lock_admission().await);
    executions
        .dispatch_pending(&record.intent.request.target_session_id, &mut admission)
        .await?;
    Ok(resolved)
}
