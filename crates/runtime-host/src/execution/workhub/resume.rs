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

use super::super::{Executions, Result, failure};
use crate::plugins::workhub::{
    control::CandidateFilter,
    resume::{Receipt, Request},
};
use maka_event_log::{message_resolution::MessageExecution, turns::InvocationState};
use maka_plugins::fiber::Context;
use maka_protocol::{OperationErrorCode as Code, turn::TurnResumeStartResult};
use maka_runtime::{
    event::{CommitError, EventWrite, Fact, InvocationOutcome, RuntimeEvent},
    input::InvocationInput,
    workhub::ResumeOrigin,
};
use std::sync::Arc;

pub(super) async fn execute(
    executions: &Arc<Executions>,
    caller: Context,
    request: Request,
    connection: uuid::Uuid,
    eligible: CandidateFilter,
) -> Result<Receipt> {
    let mut prepared = None;
    loop {
        let gate = executions.lock_admission().await;
        let _call = caller
            .admit()
            .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
        if let Some(stored) = executions
            .log
            .workhub_action(&request.action_id)
            .await
            .map_err(|e| super::commands::stored(executions, e))?
        {
            return receipt(&stored.event, &request);
        }
        if executions
            .log
            .workhub_stop(&request.action_id)
            .await
            .map_err(|e| super::commands::stored(executions, e))?
            .is_some()
            || executions
                .log
                .workhub_correction(&request.action_id)
                .await
                .map_err(|e| super::commands::stored(executions, e))?
                .is_some()
        {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub action already belongs to a control operation",
            ));
        }
        if executions.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let source = executions.workhub_source(&request.turn_id).await?;
        executions
            .workhub_target(&request.target_session_id, eligible)
            .await?
            .ok_or_else(|| {
                failure(
                    Code::OperationConflict,
                    "WorkHub resume target is unavailable",
                )
            })?;
        let delegated = executions
            .log
            .workhub_assignment(&request.delegation_action_id)
            .await
            .map_err(|e| super::commands::stored(executions, e))?
            .ok_or_else(|| {
                failure(
                    Code::OperationConflict,
                    "WorkHub resume delegation is missing",
                )
            })?;
        let delegation = delegated.delegation;
        if delegation.target.session_id != request.target_session_id {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub resume target changed",
            ));
        }
        let work = executions
            .log
            .message_execution(&request.target_session_id, &delegation.target_message_id())
            .await
            .map_err(|e| super::commands::stored(executions, e))?;
        let MessageExecution::Owned(owner) = work else {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub resume requires an exclusively owned Message",
            ));
        };
        let origin = ResumeOrigin {
            action_id: request.action_id.clone(),
            request_fingerprint: request.request_fingerprint.clone(),
            coordinator: source.invocation.clone(),
            delegation_action_id: request.delegation_action_id.clone(),
        };
        return match owner.state {
            InvocationState::Admitted | InvocationState::Running => {
                if executions
                    .active_session_owner(&owner.invocation.session_id)
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
                    .map_err(|e| failure(Code::OperationConflict, &e.to_string()))?;
                executions
                    .log
                    .append(&write)
                    .await
                    .map_err(|error| match error {
                        CommitError::OutcomeUnknown(reason) => {
                            executions.begin_drain();
                            failure(Code::CommitOutcomeUnknown, &reason)
                        }
                        CommitError::Rejected(reason) => failure(Code::OperationConflict, &reason),
                    })?;
                receipt(write.event(), &request)
            }
            InvocationState::Ended {
                outcome: InvocationOutcome::Failed { .. } | InvocationOutcome::Cancelled { .. },
                ..
            } => {
                let Some(candidate) = prepared.take() else {
                    drop(gate);
                    prepared = Some(
                        executions
                            .prepare_environment(
                                &owner.invocation.session_id,
                                Some(connection),
                                maka_client_capability::BindingMode::Strict,
                                executions
                                    .log
                                    .invocation_configuration(&owner.invocation)
                                    .await
                                    .map_err(|error| super::commands::stored(executions, error))?
                                    .map(|configuration| configuration.orchestration_mode),
                            )
                            .await,
                    );
                    continue;
                };
                let Some((environment, _input_admission)) = candidate?
                    .commit(executions, &owner.invocation.session_id)
                    .await?
                else {
                    continue;
                };
                match executions
                    .resume_workhub(origin, owner.invocation, environment)
                    .await?
                {
                    TurnResumeStartResult::Started { turn } => Ok(Receipt::Started {
                        session_id: turn.session_id,
                        turn_id: turn.turn_id,
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
        };
    }
}

fn receipt(event: &RuntimeEvent, request: &Request) -> Result<Receipt> {
    let (origin, target, receipt) = match &event.fact {
        Fact::WorkhubResumeObserved { resume, target } => (
            resume.as_ref(),
            target,
            Receipt::Running {
                session_id: target.session_id.clone(),
            },
        ),
        Fact::InvocationOpened {
            input:
                InvocationInput::Continuation {
                    workhub_resume: Some(origin),
                    ..
                },
            ..
        } => (
            origin,
            &event.invocation,
            Receipt::Started {
                session_id: event.invocation.session_id.clone(),
                turn_id: event.invocation.turn_id.clone(),
            },
        ),
        _ => {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub action belongs to another operation",
            ));
        }
    };
    if origin.request_fingerprint != request.request_fingerprint
        || origin.coordinator.turn_id != request.turn_id
        || origin.delegation_action_id != request.delegation_action_id
        || target.session_id != request.target_session_id
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub resume belongs to another request",
        ));
    }
    Ok(receipt)
}
