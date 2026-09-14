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

mod directories;
mod projection;
use super::{Host, HostError};
use crate::session::SessionConfiguration;
pub(super) use directories::Directories;
pub use directories::DirectoryRootSpec;
use maka_event_log::{
    StoreError,
    projects::{ProjectError, ProjectMutation, ProjectRegistration},
};
use maka_fs_tools::workspace::project::{self as filesystem, ProjectKind};
use maka_protocol::session::{WorkspaceProjection, WorkspaceTarget};
use maka_protocol::{Operation, OperationError, OperationErrorCode as Code, Outcome, project::*};
use serde_json::{Value, json};
use std::{path::Path, sync::atomic::Ordering};

type Result<T> = std::result::Result<T, OperationError>;

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ProjectCatalogQuery | Operation::ProjectCatalogMutate
    )
}

pub(super) fn decode_input(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    match operation {
        Operation::ProjectCatalogQuery => {
            decode_query(value)?;
        }
        Operation::ProjectCatalogMutate => {
            decode_mutation(value)?;
        }
        _ => {
            return Err(maka_protocol::ProtocolError::invalid(
                "unknown Project operation",
            ));
        }
    }
    Ok(value.clone())
}

pub(super) fn decode_output(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    match operation {
        Operation::ProjectCatalogQuery => {
            decode_query_result(value)?;
        }
        Operation::ProjectCatalogMutate => {
            decode_mutation_result(value)?;
        }
        _ => {
            return Err(maka_protocol::ProtocolError::invalid(
                "unknown Project operation",
            ));
        }
    }
    Ok(value.clone())
}

pub(super) async fn execute(
    host: &Host,
    operation: Operation,
    value: &Value,
) -> std::result::Result<Outcome, HostError> {
    let result = match operation {
        Operation::ProjectCatalogQuery => {
            let input = decode_query(value)?;
            let output = if matches!(input, Query::ListStart { .. } | Query::ListContinue { .. }) {
                match host.log.list_projects().await {
                    Ok(records) => projection::query(records, input.clone()).await,
                    Err(error) => Err(failure(Code::PersistenceFailed, error)),
                }
            } else {
                host.project_directories.query(input.clone()).await
            };
            output.and_then(|output| {
                assert_query_output(&input, &output).map_err(internal)?;
                serde_json::to_value(output).map_err(internal)
            })
        }
        Operation::ProjectCatalogMutate => mutate(host, decode_mutation(value)?)
            .await
            .and_then(|output| serde_json::to_value(output).map_err(internal)),
        _ => unreachable!("Project dispatch"),
    };
    Ok(match result {
        Ok(value) => Outcome::success(decode_output(operation, &value)?),
        Err(error) => Outcome::failure(error),
    })
}

async fn mutate(host: &Host, input: Mutation) -> Result<MutationResult> {
    let _admission = host.executions.lock_admission().await;
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let now = super::configuration::now().map_err(internal)?;
    let record = match input {
        Mutation::Register { path, prefer } => host
            .log
            .register_project(
                registration(Path::new(&path)).await?,
                prefer.unwrap_or(true),
                now,
            )
            .await
            .map_err(stored)?,
        Mutation::RegisterDirectory { root_id, segments } => {
            let (root, path) = host.project_directories.resolve(&root_id, segments).await?;
            let registration = registration(&path).await?;
            directories::validate_registration(root, Path::new(&registration.path)).await?;
            host.log
                .register_project(registration, true, now)
                .await
                .map_err(stored)?
        }
        Mutation::Relink { project_id, path } => {
            let result = host
                .log
                .relink_project::<SessionConfiguration, _>(
                    &project_id,
                    registration(Path::new(&path)).await?,
                    now,
                    |session, context| {
                        if let WorkspaceTarget::Project { project_id } =
                            &mut session.workspace.target
                        {
                            if context.previous_ids.contains(project_id) {
                                *project_id = context.project_id.clone();
                                session.workspace.host_cwd = context.destination_path.clone();
                            } else if context.absorbed_ids.contains(project_id) {
                                *project_id = context.project_id.clone();
                            }
                        }
                        Ok(())
                    },
                )
                .await
                .map_err(stored)?;
            for id in result.updated_session_ids {
                host.session_catalog
                    .publish_session(&host.changes, &id)
                    .await
                    .map_err(internal)?;
            }
            result.project
        }
        Mutation::Rename { project_id, name } => host
            .log
            .mutate_project(&project_id, ProjectMutation::Rename(name), now)
            .await
            .map_err(stored)?,
        Mutation::Archive { project_id } => host
            .log
            .mutate_project(&project_id, ProjectMutation::Archive, now)
            .await
            .map_err(stored)?,
        Mutation::Restore { project_id } => host
            .log
            .mutate_project(&project_id, ProjectMutation::Restore, now)
            .await
            .map_err(stored)?,
    };
    publish(host);
    Ok(MutationResult::Project {
        project: projection::project(record).await?,
    })
}

async fn registration(path: &Path) -> Result<ProjectRegistration> {
    let resolved = filesystem::resolve_selected(path).await.map_err(invalid)?;
    Ok(ProjectRegistration {
        identity: resolved.identity().map_err(invalid)?,
        path: filesystem::host_path(&resolved.path)
            .map_err(invalid)?
            .into(),
        is_worktree: matches!(
            resolved.kind,
            ProjectKind::Git {
                is_worktree: true,
                ..
            }
        ),
        name: resolved.name,
    })
}

/// Caller holds the existing admission gate across selection and Session commit.
pub(super) async fn resolve(host: &Host, id: &str) -> Result<WorkspaceProjection> {
    let record = host
        .log
        .get_project(id)
        .await
        .map_err(stored)?
        .ok_or_else(|| failure(Code::NotFound, "Project does not exist"))?;
    if record.archived_at.is_some() {
        return Err(failure(Code::OperationConflict, "Project is archived"));
    }
    let project_id = record.id.clone();
    let path = projection::available_paths(record)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| failure(Code::OperationConflict, "Project has no available location"))?;
    Ok(WorkspaceProjection {
        target: WorkspaceTarget::Project { project_id },
        host_cwd: path,
    })
}

pub(super) async fn record_usage(host: &Host, workspace: &WorkspaceProjection) -> Result<()> {
    if let WorkspaceTarget::Project { project_id } = &workspace.target {
        host.log
            .touch_project(
                project_id,
                &workspace.host_cwd,
                super::configuration::now().map_err(internal)?,
            )
            .await
            .map_err(stored)?;
        publish(host);
    }
    Ok(())
}

fn publish(host: &Host) {
    let revision = host.change_revision.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = host
        .changes
        .send(json!({"kind":"project.catalog.changed", "revision":revision}));
}

fn stored(error: StoreError) -> OperationError {
    let code = match error {
        StoreError::Project(ProjectError::NotFound) => Code::NotFound,
        StoreError::Project(_) => Code::OperationConflict,
        StoreError::InvalidTransition(_) => Code::InvalidRequest,
        StoreError::CommitUnknown(_) | StoreError::OperationUnknown => Code::CommitOutcomeUnknown,
        _ => Code::PersistenceFailed,
    };
    failure(code, error)
}
fn failure(code: Code, error: impl std::fmt::Display) -> OperationError {
    OperationError {
        code,
        message: error.to_string().chars().take(1024).collect(),
    }
}
fn invalid(error: impl std::fmt::Display) -> OperationError {
    failure(Code::InvalidRequest, error)
}
fn internal(error: impl std::fmt::Display) -> OperationError {
    failure(Code::InternalFailure, error)
}

pub(super) const QUERY_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::InternalFailure,
];
pub(super) const MUTATION_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::NotFound,
    Code::OperationConflict,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
];
