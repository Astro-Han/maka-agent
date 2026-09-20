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

use super::*;
use maka_event_log::workhub::correction::CorrectionResolution;
use maka_runtime::workhub::CorrectionAbort;

pub(in crate::execution::workhub) async fn settle(
    executions: &Arc<Executions>,
    identity: Identity,
) -> Result<CorrectionRecord> {
    let completed = {
        let _gate = executions.lock_admission().await;
        let record = read(executions, &identity.action_id).await?;
        validate_receipt(&identity, &record)?;
        if record.resolution.is_some() {
            return Ok(record);
        }
        if executions.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        retire(executions, &record).await?
    };
    if let Some(completed) = completed {
        completed.cancelled().await;
    }
    let record = finish(executions, &identity.action_id).await?;
    let mut admission = Some(executions.lock_admission().await);
    if let Some(CorrectionResolution::Assigned(assigned)) = &record.resolution {
        executions
            .dispatch_pending(&assigned.delegation.target.session_id, &mut admission)
            .await?;
    }
    if let Some(owner) = &record.intent.owner {
        executions
            .dispatch_pending(&owner.session_id, &mut admission)
            .await?;
    }
    Ok(record)
}

/// Before pending Messages start, old owners have passed abandoned-Run and shell
/// recovery. Durable settlement is independent of plugin activation.
pub(in crate::execution::workhub) async fn recover(executions: &Arc<Executions>) -> Result<()> {
    loop {
        let records = executions
            .log
            .pending_workhub_corrections()
            .await
            .map_err(|error| stored(executions, error))?;
        if records.is_empty() {
            return Ok(());
        }
        for record in records {
            {
                let _gate = executions.lock_admission().await;
                retire(executions, &record).await?;
            }
            finish(executions, &record.intent.request.action_id).await?;
        }
    }
}

async fn retire(
    executions: &Executions,
    record: &CorrectionRecord,
) -> Result<Option<tokio_util::sync::CancellationToken>> {
    if let Some(owner) = &record.intent.owner {
        executions
            .retire_owner(
                owner,
                maka_agent::CancellationCause::WorkhubCorrection {
                    action_id: record.intent.request.action_id.clone(),
                },
            )
            .await
    } else {
        Ok(None)
    }
}

async fn read(executions: &Executions, action_id: &ActionId) -> Result<CorrectionRecord> {
    executions
        .log
        .workhub_correction(action_id)
        .await
        .map_err(|error| stored(executions, error))?
        .ok_or_else(|| {
            failure(
                Code::InternalFailure,
                "WorkHub correction intent is missing",
            )
        })
}

async fn finish(executions: &Arc<Executions>, action_id: &ActionId) -> Result<CorrectionRecord> {
    let _gate = executions.lock_admission().await;
    let record = read(executions, action_id).await?;
    if record.resolution.is_some() {
        return Ok(record);
    }
    if executions.shutdown.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let request = &record.intent.request;
    let (delegation, configuration) = match prepare(executions, &record).await {
        Ok(prepared) => prepared,
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
            let waiting = !executions
                .log
                .pending_interactions(request.target.session_id())
                .await
                .map_err(|error| stored(executions, error))?
                .is_empty();
            return executions
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
                .map_err(|error| stored(executions, error));
        }
        Err(error) => return Err(error),
    };
    executions
        .log
        .finish_workhub_correction(action_id, delegation, configuration.as_deref())
        .await
        .map_err(|error| stored(executions, error))
}

/// Revalidate accepted execution data, never resample defaults or plugin policy.
async fn prepare(
    executions: &Arc<Executions>,
    record: &CorrectionRecord,
) -> Result<(
    Delegation,
    Option<Box<crate::session::SessionConfiguration>>,
)> {
    use maka_runtime::execution::WorkspaceTarget;
    use maka_runtime::workhub::CorrectionTarget;
    let unavailable = || {
        failure(
            Code::OperationConflict,
            "Accepted WorkHub target preparation is unavailable or changed",
        )
    };
    let request = &record.intent.request;
    let preparation: Preparation = serde_json::from_value(record.intent.preparation.clone())
        .map_err(super::super::super::internal)?;
    match (preparation, &request.target) {
        (
            Preparation::Existing {
                configuration_digest,
            },
            CorrectionTarget::Existing { .. },
        ) => {
            let owner = executions.clone();
            let current = executions
                .log
                .workhub_candidate::<crate::session::SessionConfiguration, _>(
                    request.target.session_id(),
                    move |record| {
                        record.configuration_digest == configuration_digest
                            && super::super::execution_available(&owner, record)
                    },
                )
                .await
                .map_err(|error| stored(executions, error))?
                .ok_or_else(unavailable)?;
            Ok((
                delegation(
                    executions,
                    request,
                    current.revision,
                    Some(current.configuration_digest),
                ),
                None,
            ))
        }
        (
            Preparation::Created {
                configuration,
                project_identity,
            },
            CorrectionTarget::Created { .. },
        ) => {
            crate::session::require_unmanaged(
                &executions.log,
                request.target.session_id(),
                Code::OperationConflict,
            )
            .await?;
            match (&configuration.workspace.target, project_identity) {
                (WorkspaceTarget::Project { project_id }, Some(identity)) => {
                    let project = executions
                        .log
                        .get_project(project_id)
                        .await
                        .map_err(|error| stored(executions, error))?
                        .ok_or_else(unavailable)?;
                    if project.identity != identity
                        || project.archived_at.is_some()
                        || !project
                            .locations
                            .iter()
                            .any(|location| location.path == configuration.workspace.host_cwd)
                    {
                        return Err(unavailable());
                    }
                }
                (WorkspaceTarget::HostPath { path }, None)
                    if path == &configuration.workspace.host_cwd => {}
                _ => return Err(unavailable()),
            }
            Ok((
                delegation(executions, request, 1, None),
                Some(configuration),
            ))
        }
        _ => Err(unavailable()),
    }
}
