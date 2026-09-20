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

use super::control::{Result, failure};
use crate::{execution::Creation, session::SessionConfiguration};
use maka_event_log::sessions::SessionRecord;
use maka_protocol::{OperationErrorCode as Code, session::*, workhub::ResolveResult};
use maka_runtime::{
    artifact::content_digest, execution::ToolMode, workhub::COORDINATION_SESSION_ID,
};

pub(crate) mod model;
mod workspace;

pub(crate) struct Resolution {
    pub workspace: WorkspaceProjection,
    pub creation: Option<Box<Creation>>,
    pub cancellation: tokio_util::sync::CancellationToken,
}

impl super::Control {
    pub(crate) async fn query(&self) -> Result<SessionCatalogProjection> {
        let record = self
            .commands
            .coordinator(self.caller.clone())
            .await?
            .ok_or_else(|| {
                failure(
                    Code::PersistenceFailed,
                    "WorkHub Session has not been resolved",
                )
            })?;
        Ok(crate::session::catalog_projection(record))
    }

    pub(crate) async fn resolve(
        &self,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<ResolveResult> {
        super::control::check_request(&cancellation)?;
        let _call = self
            .caller
            .admit()
            .map_err(|error| failure(Code::OperationUnavailable, error.to_string()))?;
        let existing = self.commands.coordinator(self.caller.clone()).await?;
        let workspace = workspace::prepare(&self.caller, self.workspace.clone()).await?;
        let creation = if existing.is_none() {
            let mut creation = self
                .commands
                .prepare_session(SessionCreateInput {
                    session_id: COORDINATION_SESSION_ID.into(),
                    workspace: workspace.target.clone(),
                    target: SessionCreateTarget::Model {
                        model_target: SessionModelTarget::Default,
                    },
                    mode: None,
                    name: Some("WorkHub".into()),
                    labels: None,
                    thinking_level: None,
                    tool_profile: None,
                    permission_mode: Some(PermissionMode::Bypass),
                    collaboration_mode: Some(CollaborationMode::Agent),
                    orchestration_mode: Some(behavior()),
                })
                .await
                .map_err(|error| {
                    if error.code == Code::PersistenceFailed {
                        error
                    } else {
                        failure(
                            Code::OperationConflict,
                            "WorkHub requires an available default model",
                        )
                    }
                })?;
            creation.configuration.tool_mode = ToolMode::Direct;
            creation.configuration.bound_tools = Some(super::session::tool_ceiling());
            creation.configuration.title_is_manual = false;
            Some(Box::new(creation))
        } else {
            None
        };
        self.commands
            .resolve_coordinator(
                self.caller.clone(),
                Resolution {
                    workspace,
                    creation,
                    cancellation,
                },
            )
            .await?;
        Ok(ResolveResult {
            session_id: COORDINATION_SESSION_ID.into(),
        })
    }
}

pub(crate) fn fingerprint() -> String {
    content_digest(b"maka:workhub-coordination-session:v1")
}

/// The stable create receipt proves identity; the configuration is its fixed
/// execution ceiling, not permission to use ordinary Session mutation routes.
pub(crate) fn validate(record: &SessionRecord<SessionConfiguration>) -> Result<()> {
    if record.id != COORDINATION_SESSION_ID || record.archived {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub Session identity or execution boundary changed",
        ));
    }
    validate_configuration(&record.configuration)
}

pub(crate) fn validate_workspace(workspace: &WorkspaceProjection) -> Result<()> {
    if !matches!(&workspace.target, WorkspaceTarget::HostPath { path } if path == &workspace.host_cwd)
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub workspace must be a Host directory",
        ));
    }
    Ok(())
}

pub(crate) fn validate_configuration(config: &SessionConfiguration) -> Result<()> {
    validate_workspace(&config.workspace)?;
    if config.target.model().is_none()
        || !((config.tool_profile.is_none()
            && config.orchestration_mode == behavior()
            && config.bound_tools.as_ref() == Some(&super::session::tool_ceiling()))
            || (config.tool_profile == Some(SessionToolProfile::WorkhubCoordinationV2)
                && config.orchestration_mode == BehaviorId::default()))
        || config.permission_mode != PermissionMode::Bypass
        || config.collaboration_mode != CollaborationMode::Agent
        || config.tool_mode != ToolMode::Direct
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub Session identity or execution boundary changed",
        ));
    }
    Ok(())
}

pub(crate) fn behavior() -> BehaviorId {
    super::ID
        .to_owned()
        .try_into()
        .expect("built-in behavior ID")
}

/// Upgrade only the previously validated, idle coordinator. Existing execution
/// openings keep their original facts; this does not reinterpret a live Run.
pub(crate) fn upgrade(config: &mut SessionConfiguration) {
    config.tool_profile = None;
    config.orchestration_mode = behavior();
    config.bound_tools = Some(super::session::tool_ceiling());
}
