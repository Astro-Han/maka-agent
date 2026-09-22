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
    tool_call::{ToolCallIdentity, ToolOrigin},
    tools::{ToolCallContext, ToolError, ToolJournal},
};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

impl Executions {
    /// Uses the call's admitted authority after rechecking revocation. The returned
    /// worker owns T1/effect/T2 even if the SDK caller drops its reply future.
    pub(crate) async fn plugin_file(
        self: &Arc<Self>,
        owner: Context,
        call: maka_plugins::call::Scope,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<impl Future<Output = Result<Value, ToolError>> + Send + 'static, Error> {
        let gate = self.interactions.own_admission().await;
        let lease = owner.admit().map_err(|_| Error::Revoked)?;
        let identity = owner.identity().map_err(|_| Error::Revoked)?;
        if !self.accepting() || cancellation.is_cancelled() {
            return Err(Error::Revoked);
        }
        let evidence = self.plugin_agent_evidence(&call).await?;
        let frozen = evidence.invocation.clone();
        let maka_plugins::authorization::Boundary::Session { boundary, .. } = &evidence.boundary
        else {
            return Err(Error::Denied);
        };
        let boundary = boundary.clone();
        let invocation = call.identity.agent().ok_or(Error::Denied)?.clone();
        let parent_operation_id = call.identity.operation_id().map(str::to_owned);
        let current = self
            .log
            .get_session::<SessionConfiguration>(&invocation.session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        let name = operation.name();
        let required = operation.required_tool();
        let entries = matches!(&operation, Operation::Entries(_));
        if current.archived
            || current.configuration.workspace_origin != frozen.workspace_origin
            || current.configuration.boundary_revision != boundary.boundary_revision
            || current.configuration.workspace.host_cwd != frozen.cwd
            || current
                .configuration
                .bound_tools
                .as_ref()
                .is_some_and(|tools| !tools.contains(required))
            || frozen
                .tool_composition
                .as_ref()
                .and_then(|proof| proof.bound_tools.as_ref())
                .is_some_and(|tools| !tools.contains(required))
        {
            return Err(Error::Denied);
        }
        let mode = boundary.sandbox_mode;
        let tool_use = parent_operation_id
            .as_ref()
            .map(|id| maka_runtime::tool_call::tool_use_id(&invocation.invocation_id, id));
        let grants = self
            .log
            .permission_grants(&invocation, tool_use.as_deref(), boundary.boundary_revision)
            .await
            .map_err(storage)?;
        let mut native = self
            .native_tools(
                &frozen.cwd,
                current.configuration.tool_profile,
                frozen.workspace_origin,
            )
            .await
            .map_err(|error| Error::Host(error.message))?;
        native.set = frozen
            .tool_composition
            .as_ref()
            .map(|composition| composition.native_tools)
            .unwrap_or_default();
        let (registration, directory) = tokio::task::spawn_blocking(move || {
            let identity =
                maka_fs_tools::workspace::read_identity(std::path::Path::new(&native.cwd))
                    .map_err(super::super::internal)?;
            if frozen.workspace_identity.as_ref() != Some(&identity) {
                return Err(super::super::failure(
                    maka_protocol::OperationErrorCode::OperationUnavailable,
                    "workspace identity changed",
                ));
            }
            let directory = if entries {
                let root = std::path::PathBuf::from(&native.cwd);
                let (mut sandbox, ceiling) = crate::execution::permissions::resolve(
                    mode,
                    &root,
                    &native.state_root,
                    native.workspace_origin,
                )
                .map_err(super::super::internal)?;
                for grant in &grants {
                    sandbox = sandbox
                        .with_grant(&grant.permissions, &ceiling)
                        .map_err(super::super::internal)?;
                }
                let policy = match sandbox {
                    maka_sandbox::Sandbox::Managed { filesystem, .. } => {
                        Some(filesystem.compile().map_err(super::super::internal)?)
                    }
                    _ => None,
                };
                Some((
                    maka_fs_tools::workspace::open_directory(
                        std::path::Path::new(&native.cwd),
                        &identity,
                    )
                    .map_err(super::super::internal)?,
                    root,
                    policy,
                ))
            } else {
                None
            };
            let registration = native
                .registrations_with_grants(mode, boundary.boundary_revision, &grants)?
                .into_iter()
                .find(|tool| tool.definition.name == required)
                .ok_or_else(|| {
                    super::super::failure(
                        maka_protocol::OperationErrorCode::OperationUnavailable,
                        "file operation denied",
                    )
                })?;
            Ok((registration, directory))
        })
        .await
        .map_err(|error| Error::Host(error.to_string()))?
        .map_err(|_| Error::Denied)?;
        let operation_id = uuid::Uuid::new_v4().to_string();
        let input = operation.clone().into_tool_input(&operation_id);
        let effect = if let Operation::Entries(operation) = operation {
            let (directory, root, policy) = directory.expect("captured entry-operation root");
            maka_runtime::tools::PreparedEffect::new(move |cancellation| {
                Box::pin(async move {
                    tokio::task::spawn_blocking(move || {
                        if cancellation.is_cancelled() {
                            return Err(ToolError::Failed(
                                "file operation cancelled before effect".into(),
                            ));
                        }
                        let result = maka_plugins::filesystem::entries::execute(
                            &directory,
                            operation,
                            &cancellation,
                            policy.as_ref().map(|filesystem| {
                                maka_plugins::filesystem::entries::Policy {
                                    root: &root,
                                    filesystem,
                                }
                            }),
                        )
                        .map_err(entry_error)?;
                        serde_json::to_value(result)
                            .map(Into::into)
                            .map_err(|error| ToolError::OutcomeUnknown(error.to_string()))
                    })
                    .await
                    .map_err(|error| ToolError::OutcomeUnknown(error.to_string()))?
                })
            })
        } else {
            registration
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
                .map_err(|error| Error::Invalid(error.to_string()))?
        };
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
                Err(ToolError::Persistence(_) | ToolError::CleanupUnconfirmed(_))
            ) {
                host.begin_drain();
            }
            drop(lease);
            let _ = send.send(result);
        });
        Ok(async move {
            receive
                .await
                .map_err(|_| ToolError::CleanupUnconfirmed("file worker disappeared".into()))?
        })
    }
}

pub(super) fn entry_error(error: maka_plugins::filesystem::entries::Error) -> ToolError {
    use maka_plugins::filesystem::entries::Error as File;
    match error {
        File::OutcomeUnknown(message) => ToolError::OutcomeUnknown(message),
        File::NotFound => ToolError::Io {
            kind: std::io::ErrorKind::NotFound,
            message: error.to_string(),
        },
        File::AlreadyExists => ToolError::Io {
            kind: std::io::ErrorKind::AlreadyExists,
            message: error.to_string(),
        },
        File::Io(message) => ToolError::Io {
            kind: std::io::ErrorKind::Other,
            message,
        },
        error => ToolError::Failed(error.to_string()),
    }
}
