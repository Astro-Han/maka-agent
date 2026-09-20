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

use super::Backend;
use crate::session::SessionConfiguration;
use maka_plugins::execution::{RootApproval, RootTemplate, SessionBoundary};
use maka_runtime::execution::{ModelBinding, ToolMode, WorkspaceTarget};
use maka_scheduler::{
    Error,
    authorization::{Authorization, Origin},
    task::Effect,
};

impl Backend {
    pub(super) async fn authorize(
        &self,
        origin: Origin,
        effect: Effect,
    ) -> Result<Authorization, Error> {
        let _lease = self.context.admit().map_err(failure)?;
        if !self.privacy_allows().await? {
            return Err(Error::Invalid(
                "Scheduled tasks are disabled in incognito mode".into(),
            ));
        }
        let host = self.host().map_err(failure)?;
        let (source, tool_mode) = match origin {
            Origin::User => {
                let policy = host.configuration.runtime_policy().await.map_err(failure)?;
                (
                    None,
                    if policy.policy.chat_defaults.code_mode_enabled {
                        ToolMode::CodeMode
                    } else {
                        ToolMode::Direct
                    },
                )
            }
            Origin::Agent(invocation) => {
                let frozen = host
                    .log
                    .invocation_configuration(&invocation)
                    .await
                    .map_err(failure)?
                    .ok_or_else(|| Error::Invalid("unknown scheduling invocation".into()))?;
                let commands = host
                    .authorize_plugin(
                        self.context.clone(),
                        std::slice::from_ref(&invocation.session_id),
                        &self.root_id,
                        self.context.stopping().map_err(failure)?,
                    )
                    .await
                    .map_err(failure)?;
                let boundary = commands
                    .boundaries()
                    .map_err(failure)?
                    .pop()
                    .ok_or_else(|| Error::Invalid("missing invocation boundary".into()))?;
                if boundary.permission_mode != frozen.permission_mode || boundary.cwd != frozen.cwd
                {
                    return Err(Error::Invalid(
                        "scheduling invocation authority changed".into(),
                    ));
                }
                (Some(boundary), frozen.tool_mode)
            }
        };
        match effect {
            Effect::Notify(_) => Ok(Authorization::Notification { source }),
            Effect::SessionResume { session_id } => {
                if source
                    .as_ref()
                    .is_some_and(|source| source.session_id != session_id)
                {
                    return Err(Error::Invalid(
                        "an Agent can only schedule its own Session".into(),
                    ));
                }
                let commands = host
                    .authorize_plugin(
                        self.context.clone(),
                        &[session_id],
                        &self.root_id,
                        self.context.stopping().map_err(failure)?,
                    )
                    .await
                    .map_err(failure)?;
                let boundary = commands
                    .boundaries()
                    .map_err(failure)?
                    .pop()
                    .ok_or_else(|| Error::Invalid("missing Session boundary".into()))?;
                // Do not recapture a wider grant between the two observations.
                if source.as_ref().is_some_and(|source| source != &boundary) {
                    return Err(Error::Invalid(
                        "scheduling invocation authority changed".into(),
                    ));
                }
                let session = host
                    .log
                    .get_session::<SessionConfiguration>(&boundary.session_id)
                    .await
                    .map_err(failure)?
                    .ok_or_else(|| Error::Invalid("Session was removed".into()))?;
                if session.configuration.target.model().is_none() {
                    return Err(Error::Invalid(
                        "scheduled resume requires a model Session".into(),
                    ));
                }
                Ok(Authorization::Session { boundary })
            }
            Effect::AgentRun { execution } => {
                let workspace = match execution.project_id {
                    Some(project_id) => {
                        let record = host
                            .log
                            .get_project(&project_id)
                            .await
                            .map_err(failure)?
                            .ok_or_else(|| Error::Invalid("Project does not exist".into()))?;
                        crate::server::resolve_project_workspace(record)
                            .await
                            .map_err(failure)?
                    }
                    None => {
                        let path = std::path::PathBuf::from(execution.cwd);
                        if !path.is_absolute() {
                            return Err(Error::Invalid("workspace must be absolute".into()));
                        }
                        let cwd = tokio::task::spawn_blocking(move || {
                            let canonical = path.canonicalize()?;
                            if !canonical.is_dir() {
                                return Err(std::io::Error::other("workspace is not a directory"));
                            }
                            maka_fs_tools::workspace::project::host_path(&canonical)
                                .map(str::to_owned)
                                .map_err(std::io::Error::other)
                        })
                        .await
                        .map_err(failure)?
                        .map_err(failure)?;
                        maka_protocol::session::WorkspaceProjection {
                            target: WorkspaceTarget::HostPath { path: cwd.clone() },
                            host_cwd: cwd,
                        }
                    }
                };
                if let Some(source) = &source
                    && (workspace.host_cwd != source.cwd
                        || rank(execution.permission_mode) > rank(source.permission_mode))
                {
                    return Err(Error::Invalid(
                        "scheduled execution exceeds the Agent's authority".into(),
                    ));
                }
                let model = ModelBinding {
                    connection_id: execution.llm_connection_id,
                    connection_slug: execution.llm_connection_slug,
                    model: execution.model,
                };
                crate::session::model::resolve(
                    &host.configuration,
                    &maka_protocol::session::SessionModelTarget::Explicit {
                        connection_id: model.connection_id.clone(),
                        connection_slug: model.connection_slug.clone(),
                        model: model.model.clone(),
                    },
                    execution.thinking_level,
                )
                .await
                .map_err(failure)?;
                let workspace_identity = maka_fs_tools::workspace::ensure_identity(
                    std::path::Path::new(&workspace.host_cwd),
                )
                .await
                .map_err(failure)?;
                Ok(Authorization::Root {
                    approval: Box::new(RootApproval {
                        source,
                        template: RootTemplate {
                            workspace: workspace.target,
                            cwd: workspace.host_cwd,
                            workspace_identity,
                            model,
                            thinking_level: execution.thinking_level,
                            tool_mode,
                            permission_mode: execution.permission_mode,
                            collaboration_mode: execution.collaboration_mode,
                            orchestration_mode: execution.orchestration_mode,
                        },
                    }),
                })
            }
        }
    }
    pub(super) async fn check_source(
        &self,
        source: &Option<SessionBoundary>,
    ) -> Result<(), maka_plugins::execution::CommandError> {
        let host = self.host()?;
        let commands = host.restore_plugin_authority(
            self.context.clone(),
            source.iter().cloned().collect(),
            &self.root_id,
            self.context
                .stopping()
                .map_err(|_| maka_plugins::execution::CommandError::Revoked)?,
        )?;
        commands.validate_authority().await
    }
}
fn failure(error: impl ToString) -> Error {
    Error::Unavailable(error.to_string())
}
fn rank(mode: maka_runtime::execution::PermissionMode) -> u8 {
    use maka_runtime::execution::PermissionMode;
    match mode {
        PermissionMode::Explore => 0,
        PermissionMode::Ask => 1,
        PermissionMode::Bypass => 2,
    }
}
