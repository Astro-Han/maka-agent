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
use super::Executions;
use crate::session::SessionConfiguration;
use futures_util::future::BoxFuture;
use maka_plugins::remote::{Caller, Error, SessionView, Sessions, WorkspaceViewInput, Workspaces};
use std::sync::Weak;

/// Only composition wiring holds Executions; consumers receive read-only views.
pub(crate) struct SessionViews(pub Weak<Executions>);
impl Sessions for SessionViews {
    fn read(&self, caller: Caller) -> BoxFuture<'_, Result<SessionView, Error>> {
        Box::pin(async move {
            let host = self.0.upgrade().ok_or(Error::Retired)?;
            let id = caller
                .session_id
                .ok_or_else(|| Error::Invalid("A Session is required".into()))?;
            let read = async {
                let record = host
                    .log
                    .get_session::<SessionConfiguration>(&id)
                    .await
                    .map_err(|error| Error::Provider(error.to_string()))?
                    .ok_or_else(|| Error::Invalid("Session does not exist".into()))?;
                if record.archived {
                    return Err(Error::Invalid("Session is archived".into()));
                }
                let session = record.configuration;
                let tools = if session.target.model().is_some()
                    && session.collaboration_mode
                        == maka_runtime::execution::CollaborationMode::Agent
                {
                    host.preview_tool_catalog(
                        Some(&id),
                        caller.connection_id,
                        &session.workspace.host_cwd,
                        session.permission_mode,
                        session.tool_profile,
                    )
                    .await
                    .map_err(|error| Error::Provider(error.message))?
                    .resolve_plugins()
                    .map_err(|error| Error::Provider(error.to_string()))?
                    .names()
                    .into_iter()
                    .collect()
                } else {
                    Default::default()
                };
                Ok(SessionView {
                    workspace: session.workspace,
                    tools,
                })
            };
            tokio::select! {
                biased;
                _ = caller.cancellation.cancelled() => Err(Error::Cancelled),
                result = read => result,
            }
        })
    }
}

impl Workspaces for SessionViews {
    fn read(
        &self,
        input: WorkspaceViewInput,
        caller: Caller,
    ) -> BoxFuture<'_, Result<SessionView, Error>> {
        Box::pin(async move {
            let host = self.0.upgrade().ok_or(Error::Retired)?;
            let read = async {
                use maka_runtime::execution::{CollaborationMode, WorkspaceTarget};
                let workspace = match input.workspace {
                    WorkspaceTarget::Project { project_id } => {
                        let project = host
                            .log
                            .get_project(&project_id)
                            .await
                            .map_err(|error| Error::Provider(error.to_string()))?
                            .ok_or_else(|| Error::Invalid("Project does not exist".into()))?;
                        crate::server::resolve_project_workspace(project).await
                    }
                    WorkspaceTarget::HostPath { path } => {
                        crate::server::resolve_workspace_path(path).await
                    }
                }
                .map_err(|error| Error::Invalid(error.message))?;
                let tools = if input.collaboration_mode == CollaborationMode::Agent {
                    host.preview_tool_catalog(
                        None,
                        caller.connection_id,
                        &workspace.host_cwd,
                        input.permission_mode,
                        None,
                    )
                    .await
                    .map_err(|error| Error::Provider(error.message))?
                    .resolve_plugins()
                    .map_err(|error| Error::Provider(error.to_string()))?
                    .names()
                    .into_iter()
                    .collect()
                } else {
                    Default::default()
                };
                Ok(SessionView { workspace, tools })
            };
            tokio::select! {
                biased;
                _ = caller.cancellation.cancelled() => Err(Error::Cancelled),
                result = read => result,
            }
        })
    }
}
