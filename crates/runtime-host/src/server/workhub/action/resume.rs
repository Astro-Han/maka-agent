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

use super::{ActInput, ActResult, Code, Host, OperationError, failure};
use maka_event_log::{message_resolution::MessageExecution, turns::InvocationState};
use maka_protocol::{
    turn::TurnResumeStartResult,
    workhub::{LinkedProposal, Proposal, ResumeOutcome},
};
use maka_runtime::{
    event::{CommitError, EventWrite, Fact, InvocationOutcome, RuntimeEvent},
    input::InvocationInput,
    workhub::ResumeOrigin,
};
use std::sync::Arc;

pub(super) async fn act(
    host: &Arc<Host>,
    input: ActInput,
    connection: uuid::Uuid,
) -> Result<ActResult, OperationError> {
    let Proposal::Linked(LinkedProposal::Resume {
        resumes_action_id,
        expects,
    }) = &input.proposal
    else {
        unreachable!()
    };
    let _gate = host.executions.lock_admission().await;
    let fingerprint = super::fingerprint(&input)?;
    if let Some(stored) = host
        .log
        .workhub_action(&input.action_id)
        .await
        .map_err(|e| super::stored(host, e))?
    {
        return receipt(&stored.event, &input, &fingerprint);
    }
    if host
        .log
        .workhub_stop(&input.action_id)
        .await
        .map_err(|e| super::stored(host, e))?
        .is_some()
        || host
            .log
            .workhub_correction(&input.action_id)
            .await
            .map_err(|e| super::stored(host, e))?
            .is_some()
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub action already belongs to a control operation",
        ));
    }
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let source = host.executions.workhub_source(&input.turn_id).await?;
    super::super::candidates::target(host, &expects.target_session_id)
        .await?
        .ok_or_else(|| {
            failure(
                Code::OperationConflict,
                "WorkHub resume target is unavailable",
            )
        })?;
    let delegated = host
        .log
        .workhub_assignment(resumes_action_id)
        .await
        .map_err(|e| super::stored(host, e))?
        .ok_or_else(|| {
            failure(
                Code::OperationConflict,
                "WorkHub resume delegation is missing",
            )
        })?;
    let delegation = delegated.delegation;
    if delegation.target.session_id != expects.target_session_id {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub resume target changed",
        ));
    }
    let work = host
        .log
        .message_execution(&expects.target_session_id, &delegation.target_message_id())
        .await
        .map_err(|e| super::stored(host, e))?;
    let MessageExecution::Owned(owner) = work else {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub resume requires an exclusively owned Message",
        ));
    };
    let origin = ResumeOrigin {
        action_id: input.action_id.clone(),
        request_fingerprint: fingerprint.clone(),
        coordinator: source.invocation.clone(),
        delegation_action_id: resumes_action_id.clone(),
    };
    match owner.state {
        InvocationState::Admitted | InvocationState::Running => {
            if host
                .executions
                .workhub_target(&owner.invocation.session_id)
                .as_ref()
                != Some(&owner.invocation)
            {
                return Err(failure(
                    Code::OperationUnavailable,
                    "Delegated execution owner is recovering",
                ));
            }
            let event = RuntimeEvent::new(
                source.invocation,
                Fact::WorkhubResumeObserved {
                    resume: Box::new(origin),
                    target: owner.invocation,
                },
            );
            let write = EventWrite::plain(event)
                .map_err(|e| failure(Code::OperationConflict, e.to_string()))?;
            host.log.append(&write).await.map_err(|error| match error {
                CommitError::OutcomeUnknown(reason) => {
                    host.executions.begin_drain();
                    failure(Code::CommitOutcomeUnknown, reason)
                }
                CommitError::Rejected(reason) => failure(Code::OperationConflict, reason),
            })?;
            receipt(write.event(), &input, &fingerprint)
        }
        InvocationState::Ended {
            outcome: InvocationOutcome::Failed { .. } | InvocationOutcome::Cancelled { .. },
            ..
        } => {
            match host
                .executions
                .resume_workhub(origin, owner.invocation, connection)
                .await?
            {
                TurnResumeStartResult::Started { turn } => Ok(ActResult::ResumeWork {
                    outcome: ResumeOutcome::ResumeStarted,
                    target_session_id: turn.session_id,
                    target_turn_id: Some(turn.turn_id),
                }),
                TurnResumeStartResult::Parked { .. } => Err(failure(
                    Code::OperationConflict,
                    "Delegated execution has no safe resume boundary",
                )),
            }
        }
        _ => Err(failure(
            Code::OperationConflict,
            "Delegated execution is not resumable",
        )),
    }
}

fn receipt(
    event: &RuntimeEvent,
    input: &ActInput,
    fingerprint: &str,
) -> Result<ActResult, OperationError> {
    let (origin, target, outcome) = match &event.fact {
        Fact::WorkhubResumeObserved { resume, target } => {
            (resume.as_ref(), target, ResumeOutcome::AlreadyRunning)
        }
        Fact::InvocationOpened {
            input:
                InvocationInput::Continuation {
                    workhub_resume: Some(origin),
                    ..
                },
            ..
        } => (origin, &event.invocation, ResumeOutcome::ResumeStarted),
        _ => {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub action belongs to another operation",
            ));
        }
    };
    let Proposal::Linked(LinkedProposal::Resume {
        resumes_action_id,
        expects,
    }) = &input.proposal
    else {
        unreachable!()
    };
    if origin.request_fingerprint != fingerprint
        || origin.coordinator.turn_id != input.turn_id
        || origin.delegation_action_id != *resumes_action_id
        || target.session_id != expects.target_session_id
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub resume belongs to another request",
        ));
    }
    Ok(ActResult::ResumeWork {
        outcome,
        target_session_id: target.session_id.clone(),
        target_turn_id: (outcome == ResumeOutcome::ResumeStarted).then(|| target.turn_id.clone()),
    })
}
