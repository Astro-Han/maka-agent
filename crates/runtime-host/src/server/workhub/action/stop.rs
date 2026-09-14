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

use super::{ActInput, ActResult, Code, Host, OperationError, failure, stored};
use maka_event_log::{
    StoreError,
    workhub::stop::{StopRecord, StopRequest},
};
use maka_protocol::workhub::{LinkedProposal, Proposal};
use std::sync::Arc;

pub(super) async fn act(host: &Arc<Host>, input: ActInput) -> Result<ActResult, OperationError> {
    let Proposal::Linked(LinkedProposal::Stop { expects }) = &input.proposal else {
        unreachable!()
    };
    let fingerprint = super::fingerprint(&input)?;
    let (record, completed) = {
        let _gate = host.executions.lock_admission().await;
        let record = if let Some(record) = host
            .log
            .workhub_stop(&input.action_id)
            .await
            .map_err(|error| stored(host, error))?
        {
            if record.intent.request.source.turn_id != input.turn_id
                || record.intent.request.request_fingerprint != fingerprint
                || record.intent.request.target_session_id != expects.target_session_id
            {
                return Err(failure(
                    Code::OperationConflict,
                    "WorkHub stop belongs to another request",
                ));
            }
            record
        } else {
            if host.draining.is_cancelled() {
                return Err(failure(Code::HostDraining, "Host is draining"));
            }
            let source = host.executions.workhub_source(&input.turn_id).await?;
            host.log
                .request_workhub_stop(StopRequest {
                    action_id: input.action_id,
                    request_fingerprint: fingerprint,
                    source: source.invocation,
                    target_session_id: expects.target_session_id.clone(),
                })
                .await
                .map_err(|error| stored(host, error))?
        };
        if record.resolution.is_some() {
            return receipt(record);
        }
        if host.draining.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let completed =
            host.executions
                .stop_workhub_owner(
                    record.intent.owner.as_ref().ok_or_else(|| {
                        failure(Code::InternalFailure, "Stop intent has no owner")
                    })?,
                    &record.intent.request.action_id,
                )
                .await?;
        (record, completed)
    };
    if let Some(completed) = completed {
        completed.cancelled().await;
    }
    let resolved = host
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
                stored(host, error)
            }
        })?;
    receipt(resolved)
}

fn receipt(record: StopRecord) -> Result<ActResult, OperationError> {
    let result = record
        .resolution
        .ok_or_else(|| failure(Code::InternalFailure, "Stop resolution is missing"))?;
    Ok(ActResult::StopWork {
        outcome: result.outcome,
        target_session_id: record.intent.request.target_session_id,
        target_turn_id: result.target_turn_id,
    })
}
