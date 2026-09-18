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

use super::{Error, Executions, SessionConfiguration, storage};
use maka_plugins::{fiber::Context, filesystem::Operation};
use maka_runtime::{
    event::Invocation,
    execution::PermissionMode,
    tool_call::{ToolCallIdentity, ToolOrigin},
    tools::{ToolCallContext, ToolError, ToolJournal},
};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

impl Executions {
    /// Captures the intersection of admitted and current authority. The returned
    /// worker owns T1/effect/T2 even if the SDK caller drops its reply future.
    pub(crate) async fn plugin_file(
        self: &Arc<Self>,
        owner: Context,
        invocation: Invocation,
        parent_operation_id: Option<String>,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<impl Future<Output = Result<Value, ToolError>> + Send + 'static, Error> {
        let gate = self.interactions.own_admission().await;
        let lease = owner.admit().map_err(|_| Error::Revoked)?;
        let identity = owner.identity().map_err(|_| Error::Revoked)?;
        if !self.accepting() || cancellation.is_cancelled() {
            return Err(Error::Revoked);
        }
        let frozen = self
            .log
            .invocation_configuration(&invocation)
            .await
            .map_err(storage)?
            .ok_or(Error::Denied)?;
        let current = self
            .log
            .get_session::<SessionConfiguration>(&invocation.session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        let name = operation.name();
        if current.archived
            || current.configuration.workspace.host_cwd != frozen.cwd
            || current
                .configuration
                .bound_tools
                .as_ref()
                .is_some_and(|tools| !tools.contains(name))
            || frozen
                .tool_composition
                .as_ref()
                .and_then(|proof| proof.bound_tools.as_ref())
                .is_some_and(|tools| !tools.contains(name))
        {
            return Err(Error::Denied);
        }
        let mode = match (
            frozen.permission_mode,
            current.configuration.permission_mode,
        ) {
            (PermissionMode::Explore, _) | (_, PermissionMode::Explore) => PermissionMode::Explore,
            (PermissionMode::Ask, _) | (_, PermissionMode::Ask) => PermissionMode::Ask,
            _ => PermissionMode::Bypass,
        };
        let native = self.native_tools(&frozen.cwd, current.configuration.tool_profile);
        let registration = tokio::task::spawn_blocking(move || {
            let identity =
                maka_fs_tools::workspace::read_identity(std::path::Path::new(&native.cwd))
                    .map_err(super::super::internal)?;
            if frozen.workspace_identity.as_ref() != Some(&identity) {
                return Err(super::super::failure(
                    maka_protocol::OperationErrorCode::OperationUnavailable,
                    "workspace identity changed",
                ));
            }
            native
                .registrations(mode)?
                .into_iter()
                .find(|tool| tool.definition.name == name)
                .ok_or_else(|| {
                    super::super::failure(
                        maka_protocol::OperationErrorCode::OperationUnavailable,
                        "file operation denied",
                    )
                })
        })
        .await
        .map_err(|error| Error::Host(error.to_string()))?
        .map_err(|_| Error::Denied)?;
        let operation_id = uuid::Uuid::new_v4().to_string();
        let input = operation.into_tool_input(&operation_id);
        let effect = registration
            .handler
            .prepare(
                name.into(),
                input.clone(),
                ToolCallContext {
                    invocation: invocation.clone(),
                    operation_id: operation_id.clone(),
                },
                cancellation.clone(),
            )
            .await
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let journal = ToolJournal::new(self.log.clone(), invocation);
        let host = self.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        self.workers.spawn(async move {
            // Admission captured above; do not hold global policy behind file I/O.
            drop(gate);
            let result = journal
                .invoke_prepared_call(
                    operation_id,
                    ToolCallIdentity {
                        tool_call_id: uuid::Uuid::new_v4().to_string(),
                        origin: ToolOrigin::HostSdk {
                            package_id: identity.package_id,
                            entry_id: identity.entry_id,
                            activation: identity.activation,
                            parent_operation_id,
                        },
                    },
                    name.into(),
                    input,
                    cancellation,
                    effect,
                )
                .await;
            if matches!(
                result,
                Err(ToolError::Persistence(_) | ToolError::OutcomeUnknown(_))
            ) {
                host.begin_drain();
            }
            drop(lease);
            let _ = send.send(result);
        });
        Ok(async move {
            receive
                .await
                .map_err(|_| ToolError::OutcomeUnknown("file worker disappeared".into()))?
        })
    }
}
