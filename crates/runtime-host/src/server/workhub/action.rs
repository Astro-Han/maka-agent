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
    artifact::content_digest,
    event::{CommitError, EventWrite, Fact, Invocation, RuntimeEvent},
    input::InvocationInput,
    workhub::{COORDINATION_SESSION_ID, Delegation, DelegationKind},
};
use std::sync::Arc;
use uuid::Uuid;

mod stop;
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

pub(super) async fn act(host: &Arc<Host>, input: ActInput) -> Result<ActResult, OperationError> {
    if matches!(
        &input.proposal,
        maka_protocol::workhub::Proposal::Linked(
            maka_protocol::workhub::LinkedProposal::Stop { .. }
        )
    ) {
        return stop::act(host, input).await;
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
    let _admission = host.executions.lock_admission().await;
    let fingerprint = fingerprint(&input)?;
    if host
        .log
        .workhub_stop(&input.action_id)
        .await
        .map_err(sessions::stored)?
        .is_some()
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub action already belongs to a stop",
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
        return Ok(receipt(&delegation));
    }
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
    let InvocationInput::Message { content, .. } = &source.input else {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub action requires a user message",
        ));
    };
    let target = target::prepare(host, &input, selected).await?;
    let delegation = Delegation {
        kind: target.kind(),
        action_id: input.action_id,
        request_fingerprint: fingerprint,
        source_message_event_id: source.opening_event_id,
        target: Invocation {
            session_id: target.id().to_owned(),
            turn_id: Uuid::new_v4().to_string(),
            run_id: Uuid::new_v4().to_string(),
            invocation_id: Uuid::new_v4().to_string(),
        },
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
                let code = if target.kind() == DelegationKind::Existing
                    && current.is_none_or(|record| record.revision != target.revision())
                {
                    Code::CandidateSetStale
                } else {
                    Code::OperationConflict
                };
                failure(code, reason)
            }
        });
    }
    host.executions
        .dispatch_workhub_pending(target.id())
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
        },
        DelegationKind::Created => ActResult::CreateNew {
            target_session_id,
            target_turn_id,
        },
    }
}

fn fingerprint(input: &ActInput) -> Result<String, OperationError> {
    Ok(content_digest(&serde_json::to_vec(input).map_err(
        |error| failure(Code::InternalFailure, error.to_string()),
    )?))
}
