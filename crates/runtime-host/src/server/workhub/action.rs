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

use super::{Host, failure, record, sessions};
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    workhub::{ActInput, ActResult},
};
use maka_runtime::{
    event::{CommitError, EventWrite, Fact, Invocation, RuntimeEvent},
    input::InvocationInput,
    workhub::{COORDINATION_SESSION_ID, Delegation, DelegationDelivery, DelegationKind},
};
use std::sync::Arc;
use uuid::Uuid;

mod correction;
pub(in crate::server) use correction::recover;
mod target;

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

pub(super) async fn act(
    host: &Arc<Host>,
    input: ActInput,
    connection: Uuid,
) -> Result<ActResult, OperationError> {
    if matches!(
        &input.proposal,
        maka_protocol::workhub::Proposal::Linked(
            maka_protocol::workhub::LinkedProposal::Correct { .. }
        )
    ) {
        return correction::act(host, input).await;
    }
    if matches!(
        &input.proposal,
        maka_protocol::workhub::Proposal::Linked(
            maka_protocol::workhub::LinkedProposal::Resume { .. }
        )
    ) {
        return super::control(host)?.value.resume(input, connection).await;
    }
    if matches!(
        &input.proposal,
        maka_protocol::workhub::Proposal::Linked(
            maka_protocol::workhub::LinkedProposal::Stop { .. }
        )
    ) {
        return super::control(host)?.value.stop(input).await;
    }
    admit(host, input, None).await
}

pub(super) async fn selected(
    host: &Arc<Host>,
    input: ActInput,
    selected: &super::selection::SelectedTarget,
) -> Result<ActResult, OperationError> {
    admit(host, input, Some(selected)).await
}

async fn admit(
    host: &Arc<Host>,
    input: ActInput,
    selected: Option<&super::selection::SelectedTarget>,
) -> Result<ActResult, OperationError> {
    let mut admission = Some(host.executions.lock_admission().await);
    let fingerprint = fingerprint(&input)?;
    if host
        .log
        .workhub_stop(&input.action_id)
        .await
        .map_err(sessions::stored)?
        .is_some()
        || host
            .log
            .workhub_correction(&input.action_id)
            .await
            .map_err(sessions::stored)?
            .is_some()
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub action already belongs to a control operation",
        ));
    }
    // Receipt authority survives source termination, model removal and target changes.
    if let Some(stored) = host
        .log
        .workhub_action(&input.action_id)
        .await
        .map_err(sessions::stored)?
    {
        let Fact::WorkhubDelegated { delegation } = stored.event.fact else {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub action belongs to another operation",
            ));
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
        return Ok(receipt(&delegation));
    }
    let _call = super::control(host)?
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, error.to_string()))?;
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    record(host)
        .await?
        .ok_or_else(|| failure(Code::NotFound, "WorkHub Session has not been resolved"))?;
    let source = host.executions.workhub_source(&input.turn_id).await?;
    if selected.is_some_and(|selected| selected.invocation != source.invocation) {
        return Err(failure(
            Code::OperationConflict,
            "The selecting Run is no longer active",
        ));
    }
    let InvocationInput::Message { content, .. } = source.root_input() else {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub action requires a user message",
        ));
    };
    let target = target::prepare(host, &input, selected).await?;
    let owner = host.executions.active_session_owner(target.id());
    let delivery = match (&target, &owner) {
        (
            target::Target::Existing {
                configuration_digest,
                ..
            },
            Some(_),
        ) => DelegationDelivery::Steering {
            configuration_digest: configuration_digest.clone(),
        },
        (_, None) => DelegationDelivery::NewTurn,
        (target::Target::Created { .. }, Some(_)) => {
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
        action_id: input.action_id,
        request_fingerprint: fingerprint,
        source_message_event_id: source.root_opening_event_id().to_owned(),
        target: owner.unwrap_or_else(|| Invocation {
            session_id: target.id().to_owned(),
            turn_id: Uuid::new_v4().to_string(),
            run_id: Uuid::new_v4().to_string(),
            invocation_id: Uuid::new_v4().to_string(),
        }),
        target_revision: target.revision(),
        delegation_text: input
            .delegation_text
            .unwrap_or_else(|| content.text.clone()),
    };
    delegation
        .message(content)
        .map_err(|reason| failure(Code::OperationUnavailable, reason))?;
    let result = receipt(&delegation);
    let action = EventWrite::plain(RuntimeEvent::new(
        source.invocation,
        Fact::WorkhubDelegated {
            delegation: Box::new(delegation),
        },
    ))
    .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
    let committed = match &target {
        target::Target::Created { configuration, .. } => host
            .log
            .create_workhub_session(&action, configuration)
            .await
            .map_err(|error| match error {
                maka_event_log::StoreError::CommitUnknown(_)
                | maka_event_log::StoreError::OperationUnknown => {
                    CommitError::OutcomeUnknown(error.to_string())
                }
                other => CommitError::Rejected(other.to_string()),
            }),
        target::Target::Existing { .. } => host.log.append(&action).await,
    };
    if let Err(error) = committed {
        return Err(match error {
            CommitError::OutcomeUnknown(reason) => {
                host.executions.begin_drain();
                failure(Code::CommitOutcomeUnknown, reason)
            }
            CommitError::Rejected(reason) => {
                let current = host
                    .log
                    .get_session::<crate::session::SessionConfiguration>(target.id())
                    .await
                    .map_err(sessions::stored)?;
                let code = if let target::Target::Existing {
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
                failure(code, reason)
            }
        });
    }
    host.executions
        .dispatch_pending(target.id(), &mut admission)
        .await?;
    Ok(result)
}

fn receipt(delegation: &Delegation) -> ActResult {
    let target_session_id = delegation.target.session_id.clone();
    let target_turn_id = delegation.target.turn_id.clone();
    match delegation.kind {
        DelegationKind::Existing => ActResult::DelegateExisting {
            target_session_id,
            target_turn_id,
            steered: delegation.delivery.is_steering(),
        },
        DelegationKind::Created => ActResult::CreateNew {
            target_session_id,
            target_turn_id,
        },
    }
}

fn fingerprint(input: &ActInput) -> Result<String, OperationError> {
    crate::plugins::workhub::control::fingerprint(input)
}

fn stored(host: &Host, error: maka_event_log::StoreError) -> OperationError {
    use maka_event_log::StoreError;
    match error {
        StoreError::InvalidTransition(reason) => failure(Code::OperationConflict, reason),
        StoreError::CommitUnknown(_) | StoreError::OperationUnknown => {
            host.executions.begin_drain();
            failure(Code::CommitOutcomeUnknown, error.to_string())
        }
        other => sessions::stored(other),
    }
}
