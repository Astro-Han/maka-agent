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
use crate::session::{PreparedSession, SessionConfiguration};
use maka_config::ConfigurationStore;
use maka_protocol::OperationErrorCode;
use maka_protocol::session::*;
use maka_runtime::configuration::policy::ChatDefaultPermissionMode;

pub(super) async fn create(
    host: &super::super::Host,
    input: SessionCreateInput,
) -> Result<SessionCatalogItem> {
    let _admission = host.executions.lock_admission().await;
    if host.draining.is_cancelled() {
        return Err(failure(
            OperationErrorCode::HostDraining,
            "Host is draining",
        ));
    }
    let log = &host.log;
    if input.session_id == "maka_workhub_coordination" {
        return Err(failure(
            OperationErrorCode::OperationConflict,
            "Session identity is reserved for WorkHub coordination",
        ));
    }
    if matches!(input.target, SessionCreateTarget::Executor { .. }) {
        return Err(failure(
            OperationErrorCode::OperationUnavailable,
            "Plugin executors are not implemented by this Host",
        ));
    }
    let thinking = input.thinking_level;
    let prepared = PreparedSession::new(input).map_err(invalid)?;
    let fingerprint = prepared.fingerprint();
    if let Some(record) = log
        .probe_session_create(prepared.session_id(), &fingerprint)
        .await
        .map_err(stored)?
    {
        return Ok(item(record));
    }
    let id = prepared.session_id().to_owned();
    let workspace = super::workspace::resolve(host, prepared.workspace())
        .await
        .map_err(|mut error| {
            // session.create declares conflicts, not a not_found outcome.
            if error.code == OperationErrorCode::NotFound {
                error.code = OperationErrorCode::OperationConflict;
            }
            error
        })?;
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
    Ok(item(record))
}

pub(in crate::server) async fn resolve(
    configuration: &ConfigurationStore,
    prepared: PreparedSession,
    thinking: Option<ThinkingLevel>,
    workspace: WorkspaceProjection,
) -> Result<SessionConfiguration> {
    let model = super::model::resolve(configuration, prepared.model_target(), thinking).await?;
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
    Ok(prepared.bind(workspace, model, default_permission, tool_mode))
}
