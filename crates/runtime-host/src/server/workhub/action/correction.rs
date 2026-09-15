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

use super::{ActInput, ActResult, Code, Host, OperationError, failure, stored, target};
use maka_event_log::workhub::correction::{CorrectionRecord, CorrectionResolution};
use maka_protocol::workhub::{LinkedProposal, Proposal, RoutingProposal};
use maka_runtime::workhub::ActionId;
use maka_runtime::{
    artifact::content_digest,
    event::Invocation,
    input::InvocationInput,
    workhub::{
        CorrectionAbort, CorrectionRequest, CorrectionTarget, CreateSpec, Delegation,
        DelegationDelivery,
    },
};
use std::sync::Arc;
use uuid::Uuid;

pub(super) async fn act(host: &Arc<Host>, input: ActInput) -> Result<ActResult, OperationError> {
    let (record, completed) = {
        let _gate = host.executions.lock_admission().await;
        let record = if let Some(record) = host
            .log
            .workhub_correction(&input.action_id)
            .await
            .map_err(|e| stored(host, e))?
        {
            if fingerprint(&input, &record.intent.request.target)?
                != record.intent.request.request_fingerprint
            {
                return Err(failure(
                    Code::OperationConflict,
                    "WorkHub correction belongs to another request",
                ));
            }
            verify_replay_target(host, &input, &record.intent.request.target).await?;
            record
        } else {
            if host
                .log
                .workhub_action(&input.action_id)
                .await
                .map_err(|e| stored(host, e))?
                .is_some()
                || host
                    .log
                    .workhub_stop(&input.action_id)
                    .await
                    .map_err(|e| stored(host, e))?
                    .is_some()
            {
                return Err(failure(
                    Code::OperationConflict,
                    "WorkHub action belongs to another operation",
                ));
            }
            if host.draining.is_cancelled() {
                return Err(failure(Code::HostDraining, "Host is draining"));
            }
            let source = host.executions.workhub_source(&input.turn_id).await?;
            let InvocationInput::Message { content, .. } = source.root_input() else {
                return Err(failure(
                    Code::OperationConflict,
                    "WorkHub correction requires a user message",
                ));
            };
            let prepared = target::prepare(host, &input, None).await?;
            let target = match &prepared {
                target::Target::Existing {
                    id,
                    name,
                    workspace_digest,
                    ..
                } => CorrectionTarget::Existing {
                    session_id: id.clone(),
                    name: name.clone(),
                    workspace_digest: workspace_digest.clone(),
                },
                target::Target::Created {
                    id,
                    configuration,
                    spec,
                } => CorrectionTarget::Created {
                    session_id: id.clone(),
                    name: configuration.name.clone(),
                    spec: spec.clone(),
                },
            };
            let Proposal::Linked(LinkedProposal::Correct {
                replaces_action_id, ..
            }) = &input.proposal
            else {
                unreachable!()
            };
            let request = CorrectionRequest {
                action_id: input.action_id.clone(),
                request_fingerprint: fingerprint(&input, &target)?,
                source: source.invocation.clone(),
                source_message_event_id: source.root_opening_event_id().to_owned(),
                replaces_action_id: replaces_action_id.clone(),
                target,
                delegation_text: input
                    .delegation_text
                    .clone()
                    .unwrap_or_else(|| content.text.clone()),
            };
            let preview = delegation(host, &prepared, &request);
            preview
                .message(content)
                .map_err(|e| failure(Code::OperationConflict, e))?;
            host.log
                .request_workhub_correction(
                    request,
                    matches!(&prepared, target::Target::Existing { .. })
                        .then(|| prepared.revision()),
                    host.executions.workhub_target(prepared.id()),
                )
                .await
                .map_err(|e| stored(host, e))?
        };
        if record.resolution.is_some() {
            return receipt(record);
        }
        if host.draining.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let completed = if let Some(owner) = &record.intent.owner {
            host.executions
                .retire_workhub_owner(
                    owner,
                    maka_agent::CancellationCause::WorkhubCorrection {
                        action_id: record.intent.request.action_id.clone(),
                    },
                )
                .await?
        } else {
            None
        };
        (record, completed)
    };
    if let Some(completed) = completed {
        completed.cancelled().await;
    }
    let mut admission = Some(host.executions.lock_admission().await);
    let record = finish(host, &record.intent.request.action_id).await?;
    if let Some(CorrectionResolution::Assigned(assigned)) = &record.resolution {
        host.executions
            .dispatch_pending(&assigned.delegation.target.session_id, &mut admission)
            .await?;
    }
    if let Some(owner) = &record.intent.owner {
        host.executions
            .dispatch_pending(&owner.session_id, &mut admission)
            .await?;
    }
    receipt(record)
}

/// Called before any pending Message is started. The old owners have already
/// passed ordinary abandoned-Run and shell recovery.
pub(in crate::server) async fn recover(host: &Arc<Host>) -> Result<(), OperationError> {
    let _gate = host.executions.lock_admission().await;
    loop {
        let records = host
            .log
            .pending_workhub_corrections()
            .await
            .map_err(|e| stored(host, e))?;
        if records.is_empty() {
            return Ok(());
        }
        for record in records {
            if let Some(owner) = &record.intent.owner {
                host.executions
                    .retire_workhub_owner(
                        owner,
                        maka_agent::CancellationCause::WorkhubCorrection {
                            action_id: record.intent.request.action_id.clone(),
                        },
                    )
                    .await?;
            }
            finish(host, &record.intent.request.action_id).await?;
        }
    }
}

async fn finish(
    host: &Arc<Host>,
    action_id: &ActionId,
) -> Result<CorrectionRecord, OperationError> {
    let record = host
        .log
        .workhub_correction(action_id)
        .await
        .map_err(|e| stored(host, e))?
        .ok_or_else(|| {
            failure(
                Code::InternalFailure,
                "WorkHub correction intent is missing",
            )
        })?;
    if record.resolution.is_some() {
        return Ok(record);
    }
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let request = &record.intent.request;
    let prepared = match prepare_frozen(host, request).await {
        Ok(target) => target,
        Err(error)
            if matches!(
                error.code,
                Code::OperationConflict
                    | Code::OperationUnavailable
                    | Code::NotFound
                    | Code::SessionArchived
                    | Code::CandidateSetStale
                    | Code::SessionBusy
            ) =>
        {
            let waiting = !host
                .log
                .pending_interactions(request.target.session_id())
                .await
                .map_err(|e| stored(host, e))?
                .is_empty();
            return host
                .log
                .abort_workhub_correction(
                    action_id,
                    if waiting {
                        CorrectionAbort::TargetWaitingForUser
                    } else {
                        CorrectionAbort::TargetUnavailable
                    },
                )
                .await
                .map_err(|e| stored(host, e));
        }
        Err(error) => return Err(error),
    };
    let configuration = match &prepared {
        target::Target::Created { configuration, .. } => Some(configuration.as_ref()),
        _ => None,
    };
    host.log
        .finish_workhub_correction(
            action_id,
            delegation(host, &prepared, request),
            configuration,
        )
        .await
        .map_err(|e| stored(host, e))
}

async fn prepare_frozen(
    host: &Arc<Host>,
    request: &CorrectionRequest,
) -> Result<target::Target, OperationError> {
    if let Some(spec) = request.target.create() {
        return target::prepare(
            host,
            &ActInput {
                turn_id: request.source.turn_id.clone(),
                action_id: request.action_id.clone(),
                proposal: Proposal::Route(RoutingProposal::CreateNew {
                    title: spec.title.clone(),
                }),
                candidate_set_id: None,
                create: Some(maka_protocol::workhub::CreateContext {
                    workspace: spec.workspace.clone(),
                }),
                new_work_defaults: spec.defaults.clone(),
                delegation_text: None,
            },
            None,
        )
        .await;
    }
    let record = super::super::candidates::target(host, request.target.session_id())
        .await?
        .ok_or_else(|| {
            failure(
                Code::OperationConflict,
                "WorkHub replacement target is unavailable",
            )
        })?;
    let workspace_digest =
        super::super::selection::workspace_digest(&record.configuration.workspace);
    if Some(workspace_digest.as_str()) != request.target.workspace_digest() {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub replacement target workspace changed",
        ));
    }
    Ok(target::Target::Existing {
        id: record.id,
        name: record.configuration.name,
        revision: record.revision,
        configuration_digest: record.configuration_digest,
        workspace_digest,
    })
}

fn delegation(host: &Host, target: &target::Target, request: &CorrectionRequest) -> Delegation {
    let owner = host.executions.workhub_target(target.id());
    let delivery = match (&owner, target) {
        (
            Some(_),
            target::Target::Existing {
                configuration_digest,
                ..
            },
        ) => DelegationDelivery::Steering {
            configuration_digest: configuration_digest.clone(),
        },
        _ => DelegationDelivery::NewTurn,
    };
    Delegation {
        action_id: request.action_id.clone(),
        kind: target.kind(),
        description: Some(target.description()),
        delivery,
        request_fingerprint: request.request_fingerprint.clone(),
        source_message_event_id: request.source_message_event_id.clone(),
        target: owner.unwrap_or_else(|| Invocation {
            session_id: target.id().into(),
            turn_id: Uuid::new_v4().to_string(),
            run_id: Uuid::new_v4().to_string(),
            invocation_id: Uuid::new_v4().to_string(),
        }),
        target_revision: target.revision(),
        delegation_text: request.delegation_text.clone(),
    }
}

fn receipt(record: CorrectionRecord) -> Result<ActResult, OperationError> {
    match record.resolution {
        Some(CorrectionResolution::Assigned(assigned)) => Ok(ActResult::Replace {
            replacement_disposition: match assigned.delegation.kind {
                maka_runtime::workhub::DelegationKind::Existing => {
                    maka_protocol::workhub::DelegationDisposition::DelegateExisting
                }
                maka_runtime::workhub::DelegationKind::Created => {
                    maka_protocol::workhub::DelegationDisposition::CreateNew
                }
            },
            target_session_id: assigned.delegation.target.session_id.clone(),
            target_turn_id: assigned.delegation.target.turn_id.clone(),
            steered: assigned.delegation.delivery.is_steering(),
        }),
        Some(CorrectionResolution::Aborted(_)) => Err(failure(
            Code::OperationConflict,
            "WorkHub correction retired the old association, but the replacement target is unavailable",
        )),
        None => Err(failure(
            Code::OperationUnavailable,
            "WorkHub correction is awaiting retirement",
        )),
    }
}

fn fingerprint(input: &ActInput, target: &CorrectionTarget) -> Result<String, OperationError> {
    #[derive(serde::Serialize)]
    enum Choice<'a> {
        Existing(&'a str),
        Created(CreateSpec),
    }
    let Proposal::Linked(LinkedProposal::Correct {
        replaces_action_id,
        target: route,
    }) = &input.proposal
    else {
        return Err(failure(Code::OperationConflict, "not a WorkHub correction"));
    };
    let choice = match route {
        RoutingProposal::DelegateExisting { .. } if target.create().is_none() => {
            Choice::Existing(target.session_id())
        }
        RoutingProposal::CreateNew { title } if target.create().is_some() => {
            Choice::Created(CreateSpec {
                title: title.clone(),
                workspace: input
                    .create
                    .as_ref()
                    .ok_or_else(|| {
                        failure(
                            Code::OperationConflict,
                            "WorkHub creation context is missing",
                        )
                    })?
                    .workspace
                    .clone(),
                defaults: input.new_work_defaults.clone(),
            })
        }
        _ => {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub correction target kind changed",
            ));
        }
    };
    Ok(content_digest(
        &serde_json::to_vec(&(
            "workhub.correct.v1",
            &input.action_id,
            &input.turn_id,
            replaces_action_id,
            choice,
            &input.delegation_text,
        ))
        .map_err(|e| failure(Code::InternalFailure, e.to_string()))?,
    ))
}

async fn verify_replay_target(
    host: &Arc<Host>,
    input: &ActInput,
    target: &CorrectionTarget,
) -> Result<(), OperationError> {
    let Proposal::Linked(LinkedProposal::Correct {
        target: RoutingProposal::DelegateExisting { candidate_ref },
        ..
    }) = &input.proposal
    else {
        return Ok(());
    };
    if host.draining.is_cancelled() {
        return Ok(());
    }
    let candidates = super::super::candidates::query(host).await?;
    if input.candidate_set_id.as_ref() == Some(&candidates.result.candidate_set_id)
        && candidates
            .result
            .candidates
            .iter()
            .find(|candidate| &candidate.candidate_ref == candidate_ref)
            .is_none_or(|candidate| candidate.session_id != target.session_id())
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub correction cannot redirect its admitted target",
        ));
    }
    Ok(())
}
