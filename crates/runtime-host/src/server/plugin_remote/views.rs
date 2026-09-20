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
use super::super::{Host, authority::Authority};
use crate::session::SessionConfiguration;
use futures_util::future::BoxFuture;
use maka_plugins::remote::{Access, Error, SessionView, Views, WorkspaceViewInput};
use std::sync::{Arc, Weak};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
mod read;
use read::ReadGrant;

/// Captures transport authority once; plugin-visible caller metadata is never
/// used as proof. Current credentials are checked before each read.
#[derive(Clone)]
pub(super) struct SessionViews {
    pub host: Weak<Host>,
    pub owner: maka_plugins::fiber::Context,
    pub session_id: Option<String>,
    pub connection_id: Uuid,
    pub client_instance_id: String,
    pub authority: Authority,
    pub access: Access,
    pub cancellation: CancellationToken,
    pub resources: Arc<maka_plugins::call::Resources>,
}
impl SessionViews {
    async fn files(
        &self,
        workspace: &maka_runtime::execution::WorkspaceProjection,
        session: bool,
    ) -> Result<maka_plugins::filesystem::ReadDirectory, Error> {
        let grant = ReadGrant {
            views: self.clone(),
            workspace: workspace.clone(),
            session,
        };
        grant.validate().await?;
        let root = maka_plugins::filesystem::ReadRoot::open(&workspace.host_cwd)
            .await
            .map_err(|error| Error::Provider(error.to_string()))?;
        Ok(root.bind_authorized(
            self.owner.clone(),
            self.cancellation.clone(),
            Arc::new(grant),
        ))
    }
    async fn host(&self) -> Result<Arc<Host>, Error> {
        let host = self.host.upgrade().ok_or(Error::Retired)?;
        if let Some(captured) = self.authority.credential() {
            let current = host
                .configuration
                .active_access_credentials()
                .await
                .map_err(|error| Error::Provider(error.to_string()))?
                .into_iter()
                .find(|credential| credential.credential_id == captured.credential_id)
                .ok_or(Error::Retired)?;
            let current = Authority::Managed(Box::new(current));
            current
                .validate_client(&host.configuration, &self.client_instance_id)
                .await
                .map_err(|_| Error::Retired)?;
            if !current.has_grant(maka_protocol::Operation::PluginRemote)
                || (self.access == Access::HostPaths && !current.can_use_host_paths())
            {
                return Err(Error::Retired);
            }
        }
        Ok(host)
    }
}
impl Views for SessionViews {
    fn authorize(
        &self,
        request: maka_plugins::authorization::Request,
    ) -> BoxFuture<'_, Result<maka_plugins::call::Owned, Error>> {
        Box::pin(async move {
            use maka_config::plugin_authorization::Principal;
            use maka_plugins::authorization::Target;
            let host = self.host().await?;
            let owner = self.owner.identity().map_err(|_| Error::Retired)?;
            if self.cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if let maka_plugins::composition::Scope::Session(id) = &owner.scope
                && !matches!(&request.target, Target::Session { session_id } if session_id == id)
            {
                return Err(Error::Invalid("authorization exceeds plugin scope".into()));
            }
            match &request.target {
                Target::Session { session_id } if Some(session_id) != self.session_id.as_ref() => {
                    return Err(Error::Invalid(
                        "authorization requires the caller's Session".into(),
                    ));
                }
                Target::Workspace {
                    workspace: maka_runtime::execution::WorkspaceTarget::HostPath { .. },
                    ..
                }
                | Target::Directory { .. }
                    if self.access != Access::HostPaths =>
                {
                    return Err(Error::Invalid(
                        "Remote endpoint does not allow Host paths".into(),
                    ));
                }
                _ => {}
            }
            let principal = match self.authority.credential() {
                Some(credential) => Principal::Credential {
                    credential_id: credential.credential_id.clone(),
                    client_instance_id: self.client_instance_id.clone(),
                },
                None => Principal::LocalUser {
                    client_instance_id: self.client_instance_id.clone(),
                },
            };
            host.executions
                .open_plugin_remote(
                    self.owner.clone(),
                    principal,
                    request,
                    self.connection_id,
                    self.cancellation.clone(),
                    self.resources.clone(),
                )
                .await
                .map_err(|error| Error::Provider(error.to_string()))
        })
    }
    fn session(&self) -> BoxFuture<'_, Result<SessionView, Error>> {
        Box::pin(async move {
            let _lease = self.owner.resource_call().map_err(|_| Error::Retired)?;
            let stopping = self.owner.stopping().map_err(|_| Error::Retired)?;
            let id = self
                .session_id
                .clone()
                .ok_or_else(|| Error::Invalid("A Session is required".into()))?;
            let read = async {
                let host = self.host().await?;
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
                    host.executions
                        .preview_tool_catalog(
                            Some(&id),
                            self.connection_id,
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
                    files: self.files(&session.workspace, true).await?,
                    workspace: session.workspace,
                    tools,
                })
            };
            tokio::select! {
                biased;
                _ = stopping.cancelled() => Err(Error::Retired),
                _ = self.cancellation.cancelled() => Err(Error::Cancelled),
                result = read => result,
            }
        })
    }
    fn workspace(&self, input: WorkspaceViewInput) -> BoxFuture<'_, Result<SessionView, Error>> {
        Box::pin(async move {
            let _lease = self.owner.resource_call().map_err(|_| Error::Retired)?;
            let stopping = self.owner.stopping().map_err(|_| Error::Retired)?;
            let read = async {
                let host = self.host().await?;
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
                        if self.access != Access::HostPaths {
                            return Err(Error::Invalid(
                                "Remote endpoint does not allow Host paths".into(),
                            ));
                        }
                        crate::server::resolve_workspace_path(path).await
                    }
                }
                .map_err(|error| Error::Invalid(error.message))?;
                let tools = if input.collaboration_mode == CollaborationMode::Agent {
                    host.executions
                        .preview_tool_catalog(
                            None,
                            self.connection_id,
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
                let files = self.files(&workspace, false).await?;
                Ok(SessionView {
                    workspace,
                    tools,
                    files,
                })
            };
            tokio::select! {
                biased;
                _ = stopping.cancelled() => Err(Error::Retired),
                _ = self.cancellation.cancelled() => Err(Error::Cancelled),
                result = read => result,
            }
        })
    }
}
