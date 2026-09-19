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

use super::{Result, failure, invalid, item, stored};
use crate::session::{PreparedSession, SessionConfiguration, SessionTarget};
use maka_config::ConfigurationStore;
use maka_protocol::OperationErrorCode;
use maka_protocol::session::*;
use maka_runtime::configuration::policy::ChatDefaultPermissionMode;

pub(super) async fn create(
    host: &super::super::Host,
    input: SessionCreateInput,
) -> Result<SessionCatalogItem> {
    let log = &host.log;
    crate::session::require_unmanaged(
        log,
        &input.session_id,
        OperationErrorCode::OperationConflict,
    )
    .await?;
    let thinking = input.thinking_level;
    let prepared = PreparedSession::new(input).map_err(invalid)?;
    let fingerprint = prepared.fingerprint();
    let mut observed = None;
    loop {
        let admission = host.executions.lock_admission().await;
        if host.draining.is_cancelled() {
            return Err(failure(
                OperationErrorCode::HostDraining,
                "Host is draining",
            ));
        }
        if let Some(record) = log
            .probe_session_create(prepared.session_id(), &fingerprint)
            .await
            .map_err(stored)?
        {
            return Ok(item(record));
        }
        let id = prepared.session_id().to_owned();
        let Some((project, workspace)) = observed.take() else {
            let project = match prepared.workspace() {
                WorkspaceTarget::Project { project_id } => Some(
                    log.get_project(project_id)
                        .await
                        .map_err(stored)?
                        .ok_or_else(|| {
                            failure(
                                OperationErrorCode::OperationConflict,
                                "Project does not exist",
                            )
                        })?,
                ),
                WorkspaceTarget::HostPath { .. } => None,
            };
            drop(admission);
            let workspace = match &project {
                Some(record) => super::super::projects::resolve_record(record.clone()).await,
                None => super::workspace::resolve(host, prepared.workspace()).await,
            };
            observed = Some((project, workspace));
            continue;
        };
        if let Some(project) = project
            && log.get_project(&project.id).await.map_err(stored)?.as_ref() != Some(&project)
        {
            continue;
        }
        let workspace = workspace.map_err(|mut error| {
            // session.create declares conflicts, not a not_found outcome.
            if error.code == OperationErrorCode::NotFound {
                error.code = OperationErrorCode::OperationConflict;
            }
            error
        })?;
        if let SessionCreateTarget::Executor { executor_id } = prepared.target() {
            host.executions.executor_binding(&id, executor_id)?;
        }
        let config = resolve(&host.configuration, prepared, thinking, workspace).await?;
        super::super::projects::record_usage(host, &config.workspace).await?;
        let record = log
            .create_session(
                &id,
                &fingerprint,
                &config,
                super::super::configuration::now().map_err(super::super::configuration::failure)?,
            )
            .await
            .map_err(stored)?;
        return Ok(item(record));
    }
}

pub(crate) async fn resolve(
    configuration: &ConfigurationStore,
    prepared: PreparedSession,
    thinking: Option<ThinkingLevel>,
    workspace: WorkspaceProjection,
) -> Result<SessionConfiguration> {
    let target = match prepared.target() {
        SessionCreateTarget::Model { model_target } => SessionTarget::Model {
            model: super::model::resolve(configuration, model_target, thinking).await?,
        },
        SessionCreateTarget::Executor { executor_id } => SessionTarget::Executor {
            executor_id: executor_id.clone(),
        },
    };
    let policy = configuration
        .runtime_policy()
        .await
        .map_err(super::super::configuration::failure)?;
    let default_permission = match policy.policy.chat_defaults.permission_mode {
        ChatDefaultPermissionMode::Ask => PermissionMode::Ask,
        ChatDefaultPermissionMode::Bypass => PermissionMode::Bypass,
    };
    // Thinking defaults are applied by the client composer. Omission here also
    // represents its explicit "model default" choice and must remain unchanged.
    let tool_mode = if policy.policy.chat_defaults.code_mode_enabled {
        maka_runtime::execution::ToolMode::CodeMode
    } else {
        maka_runtime::execution::ToolMode::Direct
    };
    Ok(prepared.bind(workspace, target, default_permission, tool_mode))
}
