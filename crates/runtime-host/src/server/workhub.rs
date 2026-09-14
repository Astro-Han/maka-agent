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

use super::{Host, HostError, configuration, sessions};
use crate::session::SessionConfiguration;
use maka_event_log::sessions::{SessionExecutionState, SessionRecord};
use maka_protocol::{
    Operation, OperationError, OperationErrorCode as Code, Outcome, session::*, workhub,
};
use maka_runtime::{execution::ToolMode, workhub::COORDINATION_SESSION_ID};
use serde_json::Value;

mod action;
mod candidates;
mod selection;
pub(super) use action::ERRORS as ACTION_ERRORS;
pub(super) use candidates::ERRORS as CANDIDATE_ERRORS;

pub(super) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::OperationConflict,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
];
const WORKSPACE: &str = "workhub-coordination";

pub(super) const TURN_ERRORS: &[Code] = &[
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
];

pub(super) async fn execute(
    host: &std::sync::Arc<Host>,
    operation: Operation,
    value: &Value,
    connection_id: uuid::Uuid,
) -> Result<Outcome, HostError> {
    let result = match operation {
        Operation::WorkhubCoordinationResolve => resolve(host).await.and_then(|()| {
            serialize(workhub::ResolveResult {
                session_id: COORDINATION_SESSION_ID.into(),
            })
        }),
        Operation::WorkhubCoordinationQuery => query(host).await.and_then(serialize),
        Operation::WorkhubCoordinationActFromTurn => {
            action::act(host, workhub::decode_act(value)?, connection_id)
                .await
                .and_then(serialize)
        }
        Operation::WorkhubCoordinationSelectAndDelegate => {
            selection::select(host, workhub::decode_selection(value)?)
                .await
                .and_then(serialize)
        }
        Operation::WorkhubCoordinationCandidates => candidates::query(host)
            .await
            .and_then(|candidates| serialize(candidates.result)),
        Operation::WorkhubCoordinationAnswer => {
            let input = workhub::decode_answer_input(value)?;
            answer(host, input, connection_id).await.and_then(serialize)
        }
        Operation::WorkhubCoordinationConfigureModel => {
            let input = workhub::decode_model_input(value)?;
            sessions::configuration::apply(host, input)
                .await
                .and_then(serialize)
        }
        _ => unreachable!("validated WorkHub control operation"),
    };
    match result {
        Ok(value) => {
            workhub::decode_output(operation, &value)?;
            if !matches!(
                operation,
                Operation::WorkhubCoordinationQuery | Operation::WorkhubCoordinationCandidates
            ) {
                host.session_catalog
                    .publish_session(&host.changes, COORDINATION_SESSION_ID)
                    .await?;
            }
            Ok(Outcome::success(value))
        }
        Err(error) => Ok(Outcome::failure(error)),
    }
}

async fn answer(
    host: &std::sync::Arc<Host>,
    input: workhub::AnswerInput,
    connection_id: uuid::Uuid,
) -> Result<workhub::TurnResult, OperationError> {
    let _admission = host.executions.lock_admission().await;
    let session = record(host)
        .await?
        .ok_or_else(|| failure(Code::NotFound, "WorkHub Session has not been resolved"))?;
    host.executions
        .answer_workhub(input, session, connection_id, host.root_id())
        .await
}

fn serialize(value: impl serde::Serialize) -> Result<Value, OperationError> {
    serde_json::to_value(value).map_err(|e| failure(Code::InternalFailure, e.to_string()))
}

fn fingerprint() -> String {
    maka_runtime::artifact::content_digest(b"maka:workhub-coordination-session:v1")
}

/// The stable create fingerprint is the identity proof; profile is its fixed execution ceiling.
pub(super) fn validate(record: &SessionRecord<SessionConfiguration>) -> Result<(), OperationError> {
    let config = &record.configuration;
    if record.id != COORDINATION_SESSION_ID
        || record.archived
        || !matches!(config.workspace.target, WorkspaceTarget::HostPath { .. })
        || config.tool_profile != Some(SessionToolProfile::WorkhubCoordinationV2)
        || config.permission_mode != PermissionMode::Bypass
        || config.collaboration_mode != CollaborationMode::Agent
        || config.orchestration_mode != OrchestrationMode::Default
        || config.tool_mode != ToolMode::Direct
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub Session identity or execution boundary changed",
        ));
    }
    Ok(())
}

pub(super) async fn record(
    host: &Host,
) -> Result<Option<SessionRecord<SessionConfiguration>>, OperationError> {
    let record = host
        .log
        .probe_session_create(COORDINATION_SESSION_ID, &fingerprint())
        .await
        .map_err(sessions::stored)?;
    if let Some(record) = &record {
        validate(record)?;
    }
    Ok(record)
}

async fn query(host: &Host) -> Result<SessionCatalogItem, OperationError> {
    let record = record(host).await?.ok_or_else(|| {
        failure(
            Code::PersistenceFailed,
            "WorkHub Session has not been resolved",
        )
    })?;
    Ok(SessionCatalogItem::Projection(Box::new(
        sessions::projection::project(record),
    )))
}

async fn resolve(host: &Host) -> Result<(), OperationError> {
    let _admission = host.executions.lock_admission().await;
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let existing = record(host).await?;
    let path = host.root.canonical_path().join(WORKSPACE);
    match tokio::fs::create_dir(&path).await {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(failure(Code::PersistenceFailed, e.to_string())),
    }
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|e| failure(Code::PersistenceFailed, e.to_string()))?;
    if !metadata.file_type().is_dir() {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub workspace must be a real directory",
        ));
    }
    let cwd = path
        .to_str()
        .ok_or_else(|| failure(Code::OperationConflict, "WorkHub workspace is not UTF-8"))?
        .to_owned();
    let workspace = WorkspaceProjection {
        target: WorkspaceTarget::HostPath { path: cwd.clone() },
        host_cwd: cwd,
    };
    if let Some(record) = existing {
        if record.configuration.workspace != workspace {
            if host
                .executions
                .has_session_work(COORDINATION_SESSION_ID)
                .await
                .map_err(sessions::stored)?
                || record.execution.as_ref().is_some_and(|execution| {
                    matches!(execution.state, SessionExecutionState::Live { .. })
                })
            {
                return Err(failure(
                    Code::OperationUnavailable,
                    "WorkHub workspace cannot change while executing",
                ));
            }
            let result = host
                .log
                .update_session_metadata(
                    COORDINATION_SESSION_ID,
                    record.revision,
                    move |config: &mut SessionConfiguration| {
                        config.workspace = workspace;
                        Ok(())
                    },
                )
                .await
                .map_err(sessions::stored)?;
            if matches!(
                sessions::mutation::result(result),
                SessionUpdateResult::RevisionConflict { .. }
            ) {
                return Err(failure(
                    Code::OperationConflict,
                    "WorkHub Session changed during resolution",
                ));
            }
        }
        return Ok(());
    }
    let model = sessions::model::resolve(&host.configuration, &SessionModelTarget::Default, None)
        .await
        .map_err(|e| {
            if e.code == Code::PersistenceFailed {
                e
            } else {
                failure(
                    Code::OperationConflict,
                    "WorkHub requires an available default model",
                )
            }
        })?;
    let config = SessionConfiguration {
        workspace,
        name: "WorkHub".into(),
        labels: Vec::new(),
        is_flagged: false,
        title_is_manual: false,
        model,
        connection_locked: false,
        thinking_level: None,
        tool_profile: Some(SessionToolProfile::WorkhubCoordinationV2),
        tool_mode: ToolMode::Direct,
        permission_mode: PermissionMode::Bypass,
        boundary_revision: 0,
        collaboration_mode: CollaborationMode::Agent,
        orchestration_mode: OrchestrationMode::Default,
    };
    let record = host
        .log
        .create_session(
            COORDINATION_SESSION_ID,
            &fingerprint(),
            &config,
            configuration::now().map_err(configuration::failure)?,
        )
        .await
        .map_err(sessions::stored)?;
    validate(&record)
}

fn failure(code: Code, message: impl Into<String>) -> OperationError {
    OperationError {
        code,
        message: message.into().chars().take(1024).collect(),
    }
}
