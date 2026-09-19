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

mod receipt;
use super::super::{Executions, Result, failure};
use super::commands::WorkHubCommands;
use crate::plugins::workhub::{control::Identity, delegation::Request, target::Target};
use maka_plugins::fiber::Context;
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::{
    event::{CommitError, EventWrite, Fact, Invocation, RuntimeEvent},
    input::InvocationInput,
    workhub::{Delegation, DelegationDelivery},
};
use std::sync::Arc;
use uuid::Uuid;

pub(super) async fn probe(
    executions: &Arc<Executions>,
    caller: Context,
    identity: Identity,
) -> Result<Option<Delegation>> {
    let _gate = executions.lock_admission().await;
    let _call = caller
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    let receipt = receipt::read(executions, &identity).await?;
    if receipt.is_none() {
        if executions.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let source = executions.workhub_source(&identity.turn_id).await?;
        if !matches!(source.root_input(), InvocationInput::Message { .. }) {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub action requires a user message",
            ));
        }
    }
    Ok(receipt)
}

pub(super) async fn execute(
    commands: &WorkHubCommands,
    executions: &Arc<Executions>,
    caller: Context,
    request: Request,
) -> Result<Delegation> {
    let mut admission = Some(executions.lock_admission().await);
    let _call = caller
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    if let Some(receipt) = receipt::read(executions, &request.identity).await? {
        return Ok(receipt);
    }
    if executions.shutdown.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let source = executions.workhub_source(&request.identity.turn_id).await?;
    if let Some(selected) = &request.selected {
        if selected.invocation != source.invocation {
            return Err(failure(
                Code::OperationConflict,
                "The selecting Run is no longer active",
            ));
        }
        crate::plugins::workhub::target::validate_selection(selected)?;
    }
    let InvocationInput::Message { content, .. } = source.root_input() else {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub action requires a user message",
        ));
    };
    let target = request.target;
    commands.validate_target(executions, &target).await?;
    let owner = executions.active_session_owner(target.id());
    let delivery = match (&target, &owner) {
        (
            Target::Existing {
                configuration_digest,
                ..
            },
            Some(_),
        ) => DelegationDelivery::Steering {
            configuration_digest: configuration_digest.clone(),
        },
        (_, None) => DelegationDelivery::NewTurn,
        (Target::Created { .. }, Some(_)) => {
            return Err(failure(
                Code::OperationConflict,
                "Created target already has an execution owner",
            ));
        }
    };
    let delegation = Delegation {
        kind: target.kind(),
        description: Some(target.description()),
        delivery,
        action_id: request.identity.action_id,
        request_fingerprint: request.identity.fingerprint,
        source_message_event_id: source.root_opening_event_id().to_owned(),
        target: owner.unwrap_or_else(|| Invocation {
            session_id: target.id().to_owned(),
            turn_id: Uuid::new_v4().to_string(),
            run_id: Uuid::new_v4().to_string(),
            invocation_id: Uuid::new_v4().to_string(),
        }),
        target_revision: target.revision(),
        delegation_text: request.text.unwrap_or_else(|| content.text.clone()),
    };
    delegation
        .message(content)
        .map_err(|reason| failure(Code::OperationUnavailable, reason))?;
    let action = EventWrite::plain(RuntimeEvent::new(
        source.invocation,
        Fact::WorkhubDelegated {
            delegation: Box::new(delegation.clone()),
        },
    ))
    .map_err(|error| failure(Code::InternalFailure, &error.to_string()))?;
    let committed = match &target {
        Target::Created { creation, .. } => executions
            .log
            .create_workhub_session(&action, &creation.configuration)
            .await
            .map_err(|error| match error {
                maka_event_log::StoreError::CommitUnknown(_)
                | maka_event_log::StoreError::OperationUnknown => {
                    CommitError::OutcomeUnknown(error.to_string())
                }
                other => CommitError::Rejected(other.to_string()),
            }),
        Target::Existing { .. } => executions.log.append(&action).await,
    };
    if let Err(error) = committed {
        return Err(match error {
            CommitError::OutcomeUnknown(reason) => {
                executions.begin_drain();
                failure(Code::CommitOutcomeUnknown, &reason)
            }
            CommitError::Rejected(reason) => {
                let current = executions
                    .log
                    .get_session::<crate::session::SessionConfiguration>(target.id())
                    .await
                    .map_err(|error| super::commands::stored(executions, error))?;
                let code = if let Target::Existing {
                    configuration_digest,
                    ..
                } = &target
                    && current.is_none_or(|record| {
                        record.archived || record.configuration_digest != *configuration_digest
                    }) {
                    Code::CandidateSetStale
                } else {
                    Code::OperationConflict
                };
                failure(code, &reason)
            }
        });
    }
    executions
        .dispatch_pending(target.id(), &mut admission)
        .await?;
    Ok(delegation)
}
