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

use super::{Host, candidates, failure, record, sessions};
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    workhub::{ActInput, ActResult, Proposal, RoutingProposal},
};
use maka_runtime::{
    artifact::content_digest,
    event::{CommitError, EventWrite, Fact, Invocation, RuntimeEvent},
    input::InvocationInput,
    workhub::{COORDINATION_SESSION_ID, Delegation},
};
use std::sync::Arc;
use uuid::Uuid;

pub(in crate::server) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::NotFound,
    Code::SessionArchived,
    Code::SessionBusy,
    Code::OperationConflict,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
    Code::CandidateSetStale,
];

pub(super) async fn act(host: &Arc<Host>, input: ActInput) -> Result<ActResult, OperationError> {
    let _admission = host.executions.lock_admission().await;
    let fingerprint = content_digest(
        &serde_json::to_vec(&input)
            .map_err(|error| failure(Code::InternalFailure, error.to_string()))?,
    );
    // Receipt authority survives source termination, model removal and target changes.
    if let Some(stored) = host
        .log
        .workhub_action(&input.action_id)
        .await
        .map_err(sessions::stored)?
    {
        let Fact::WorkhubDelegated { delegation } = stored.event.fact else {
            unreachable!()
        };
        if stored.event.invocation.session_id != COORDINATION_SESSION_ID
            || stored.event.invocation.turn_id != input.turn_id
            || delegation.request_fingerprint != fingerprint
        {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub action belongs to another request",
            ));
        }
        return Ok(receipt(&delegation.target));
    }
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    record(host)
        .await?
        .ok_or_else(|| failure(Code::NotFound, "WorkHub Session has not been resolved"))?;
    let Proposal::Route(RoutingProposal::DelegateExisting { candidate_ref }) = &input.proposal
    else {
        return Err(failure(
            Code::OperationUnavailable,
            "This WorkHub action is not installed",
        ));
    };
    let source = host.executions.workhub_source(&input.turn_id).await?;
    let InvocationInput::Message { content, .. } = &source.input else {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub action requires a user message",
        ));
    };
    let candidates = candidates::query(host).await?;
    if input.candidate_set_id.as_ref() != Some(&candidates.result.candidate_set_id) {
        return Err(failure(
            Code::CandidateSetStale,
            "WorkHub candidate set changed",
        ));
    }
    let target = candidates
        .result
        .candidates
        .iter()
        .zip(candidates.records)
        .find_map(|(candidate, record)| {
            (&candidate.candidate_ref == candidate_ref).then_some(record)
        })
        .ok_or_else(|| {
            failure(
                Code::CandidateSetStale,
                "WorkHub candidate is no longer eligible",
            )
        })?;
    let delegation = Delegation {
        action_id: input.action_id,
        request_fingerprint: fingerprint,
        source_message_event_id: source.opening_event_id,
        target: Invocation {
            session_id: target.id.clone(),
            turn_id: Uuid::new_v4().to_string(),
            run_id: Uuid::new_v4().to_string(),
            invocation_id: Uuid::new_v4().to_string(),
        },
        target_revision: target.revision,
        delegation_text: input
            .delegation_text
            .unwrap_or_else(|| content.text.clone()),
    };
    delegation
        .message(content)
        .map_err(|reason| failure(Code::OperationUnavailable, reason))?;
    let result = receipt(&delegation.target);
    let action = EventWrite::plain(RuntimeEvent::new(
        source.invocation,
        Fact::WorkhubDelegated {
            delegation: Box::new(delegation),
        },
    ))
    .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
    if let Err(error) = host.log.append(&action).await {
        return Err(match error {
            CommitError::OutcomeUnknown(reason) => {
                host.executions.begin_drain();
                failure(Code::CommitOutcomeUnknown, reason)
            }
            CommitError::Rejected(reason) => {
                let current = host
                    .log
                    .get_session::<crate::session::SessionConfiguration>(&target.id)
                    .await
                    .map_err(sessions::stored)?;
                let code = if current.is_none_or(|record| record.revision != target.revision) {
                    Code::CandidateSetStale
                } else {
                    Code::OperationConflict
                };
                failure(code, reason)
            }
        });
    }
    host.executions.dispatch_workhub_pending(&target.id).await?;
    Ok(result)
}

fn receipt(target: &Invocation) -> ActResult {
    ActResult::DelegateExisting {
        target_session_id: target.session_id.clone(),
        target_turn_id: target.turn_id.clone(),
    }
}
