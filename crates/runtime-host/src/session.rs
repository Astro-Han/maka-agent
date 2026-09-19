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

//! Session metadata policy. Execution content remains in the runtime log.
mod constraints;
mod metadata;
pub(crate) mod model;
mod name;
mod projection;
pub use projection::catalog_projection;

pub use metadata::apply_metadata_patch;

use maka_event_log::sessions::SessionRecord;
use maka_protocol::session::*;
use maka_protocol::{ProtocolError, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

pub use maka_runtime::execution::ModelBinding as SessionModel;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum SessionTarget {
    Model {
        model: SessionModel,
    },
    Executor {
        executor_id: maka_runtime::executor::ExecutorId,
    },
}
impl SessionTarget {
    pub fn model(&self) -> Option<&SessionModel> {
        match self {
            Self::Model { model } => Some(model),
            Self::Executor { .. } => None,
        }
    }
}
impl From<SessionModel> for SessionTarget {
    fn from(model: SessionModel) -> Self {
        Self::Model { model }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionConfiguration {
    pub workspace: WorkspaceProjection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<maka_fs_tools::worktree::Binding>,
    pub name: String,
    pub labels: Vec<String>,
    #[serde(default)]
    pub is_flagged: bool,
    #[serde(default)]
    pub title_is_manual: bool,
    #[serde(flatten)]
    pub target: SessionTarget,
    #[serde(default)]
    pub connection_locked: bool,
    pub thinking_level: Option<ThinkingLevel>,
    pub tool_profile: Option<SessionToolProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_tools: Option<std::collections::BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Frozen at creation. Missing on older Rust Sessions means direct tools.
    #[serde(default)]
    pub tool_mode: maka_runtime::execution::ToolMode,
    pub permission_mode: PermissionMode,
    /// Revision of the enforced policy, independent of unrelated catalog changes.
    #[serde(default)]
    pub boundary_revision: u64,
    pub collaboration_mode: CollaborationMode,
    pub orchestration_mode: BehaviorId,
}

impl SessionConfiguration {
    pub async fn invocation_configuration(
        &self,
    ) -> std::io::Result<maka_runtime::execution::InvocationConfiguration> {
        let workspace_identity = maka_fs_tools::workspace::ensure_identity(std::path::Path::new(
            &self.workspace.host_cwd,
        ))
        .await?;
        Ok(self.observed_configuration(workspace_identity))
    }

    pub(crate) fn observed_configuration(
        &self,
        workspace_identity: maka_runtime::execution::WorkspaceIdentity,
    ) -> maka_runtime::execution::InvocationConfiguration {
        maka_runtime::execution::InvocationConfiguration {
            system_prompt: None,
            tool_composition: None,
            cwd: self.workspace.host_cwd.clone(),
            workspace_identity: Some(workspace_identity),
            permission_mode: self.permission_mode,
            collaboration_mode: self.collaboration_mode,
            orchestration_mode: self.orchestration_mode.clone(),
            tool_mode: self.tool_mode,
            model: self.target.model().cloned(),
            thinking_level: self.thinking_level,
        }
    }
}

/// Preparing the stable request identity precedes model/workspace resolution.
/// An exact replay can therefore succeed even if its old connection was removed.
pub struct PreparedSession {
    session_id: String,
    workspace: WorkspaceTarget,
    target: SessionCreateTarget,
    name: String,
    labels: Vec<String>,
    permission_mode: Option<PermissionMode>,
    thinking_level: Option<ThinkingLevel>,
    tool_profile: Option<SessionToolProfile>,
    collaboration_mode: CollaborationMode,
    orchestration_mode: BehaviorId,
}

impl PreparedSession {
    pub fn new(input: SessionCreateInput) -> Result<Self> {
        if matches!(input.target, SessionCreateTarget::Executor { .. })
            && (input.thinking_level.is_some()
                || input.tool_profile.is_some()
                || input.mode.is_some()
                || input
                    .orchestration_mode
                    .as_ref()
                    .is_some_and(|mode| mode != &BehaviorId::default())
                || input
                    .collaboration_mode
                    .is_some_and(|mode| mode != CollaborationMode::Agent))
        {
            return Err(ProtocolError::invalid(
                "Executor Sessions do not accept native model or orchestration settings",
            ));
        }
        if input.labels.as_ref().is_some_and(|labels| {
            labels
                .iter()
                .any(|label| matches!(label.as_str(), "mode:bot" | "mode:deep_research"))
        }) {
            return Err(ProtocolError::invalid(
                "Session creation cannot set reserved execution labels",
            ));
        }
        if input.mode.is_none() && input.permission_mode == Some(PermissionMode::Explore) {
            return Err(ProtocolError::invalid(
                "Explore permission requires a declared Session mode",
            ));
        }
        let requested_name = match input.mode {
            Some(SessionStartMode::DeepResearch) => "Deep Research",
            _ => input.name.as_deref().unwrap_or("New Chat"),
        };
        let name = name::normalize(requested_name)?;
        let mut labels = input.labels.clone().unwrap_or_default();
        match input.mode {
            Some(SessionStartMode::DeepResearch) => labels.push("mode:deep_research".into()),
            Some(SessionStartMode::Bot) => labels.push("mode:bot".into()),
            None => {}
        }
        let permission_mode = if input.mode.is_some() {
            Some(PermissionMode::Explore)
        } else {
            input.permission_mode
        };
        Ok(Self {
            session_id: input.session_id,
            workspace: input.workspace,
            target: input.target,
            name,
            labels,
            permission_mode,
            thinking_level: input.thinking_level,
            tool_profile: input.tool_profile,
            collaboration_mode: input.collaboration_mode.unwrap_or(CollaborationMode::Agent),
            orchestration_mode: input.orchestration_mode.unwrap_or_default(),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn workspace(&self) -> &WorkspaceTarget {
        &self.workspace
    }
    pub fn target(&self) -> &SessionCreateTarget {
        &self.target
    }

    pub fn fingerprint(&self) -> String {
        let workspace = match &self.workspace {
            WorkspaceTarget::HostPath { path } => json!(["host_path", path]),
            WorkspaceTarget::Project { project_id } => json!(["project", project_id]),
        };
        let model = match &self.target {
            SessionCreateTarget::Executor { executor_id } => json!(["executor", executor_id]),
            SessionCreateTarget::Model {
                model_target: SessionModelTarget::Default,
            } => json!(["default"]),
            SessionCreateTarget::Model {
                model_target:
                    SessionModelTarget::Explicit {
                        connection_id,
                        connection_slug,
                        model,
                    },
            } => json!([connection_id, connection_slug, model]),
        };
        let permission = self
            .permission_mode
            .map(|mode| json!(mode))
            .unwrap_or_else(|| json!(["runtime_default"]));
        let identity = json!([
            "session.create.v4",
            self.session_id,
            workspace,
            self.name,
            self.labels,
            model,
            self.thinking_level,
            self.tool_profile,
            permission,
            self.collaboration_mode,
            self.orchestration_mode,
        ]);
        format!(
            "sha256:{:x}",
            Sha256::digest(identity.to_string().as_bytes())
        )
    }

    pub fn bind(
        self,
        workspace: WorkspaceProjection,
        target: impl Into<SessionTarget>,
        default_permission: PermissionMode,
        tool_mode: maka_runtime::execution::ToolMode,
    ) -> SessionConfiguration {
        SessionConfiguration {
            workspace,
            worktree: None,
            name: self.name,
            labels: self.labels,
            is_flagged: false,
            title_is_manual: false,
            target: target.into(),
            connection_locked: false,
            thinking_level: self.thinking_level,
            tool_profile: self.tool_profile,
            bound_tools: None,
            instructions: None,
            tool_mode,
            permission_mode: self.permission_mode.unwrap_or(default_permission),
            boundary_revision: 0,
            collaboration_mode: self.collaboration_mode,
            orchestration_mode: self.orchestration_mode,
        }
    }
}

/// Durable control metadata baseline. The host overlays execution status from
/// canonical runtime facts before presenting a live catalog entry.
pub fn metadata_projection(
    record: SessionRecord<SessionConfiguration>,
) -> SessionCatalogProjection {
    let config = record.configuration;
    let mut labels = Vec::new();
    let mut labels_truncated = false;
    for label in config.labels {
        if labels.len() >= 32
            || label.is_empty()
            || label.len() > 128
            || label.trim() != label
            || label.chars().any(|ch| ch <= '\u{1f}' || ch == '\u{7f}')
            || labels.contains(&label)
        {
            labels_truncated = true;
        } else {
            labels.push(label);
        }
    }
    let (backend, executor_id, connection_id, connection_slug, model) = match config.target {
        SessionTarget::Model { model } => (
            Backend::AiSdk,
            None,
            Some(model.connection_id),
            model.connection_slug,
            model.model,
        ),
        SessionTarget::Executor { executor_id } => {
            let name = executor_id.as_str().to_owned();
            (
                Backend::PluginExecutor,
                Some(executor_id),
                None,
                format!("executor:{name}"),
                name,
            )
        }
    };
    SessionCatalogProjection {
        id: record.id,
        revision: record.revision,
        workspace: config.workspace,
        created_at: record.created_at,
        activity_at: record.updated_at,
        name: config.name,
        is_flagged: config.is_flagged,
        is_archived: record.archived,
        labels,
        labels_truncated,
        has_unread: record.read_state.has_unread,
        status: SessionStatus::Active,
        backend,
        executor_id,
        llm_connection_id: connection_id,
        llm_connection_slug: connection_slug,
        connection_locked: config.connection_locked,
        model,
        permission_mode: config.permission_mode,
        collaboration_mode: config.collaboration_mode,
        orchestration_mode: config.orchestration_mode,
        thinking_level: config.thinking_level,
        last_message_at: None,
        last_message_preview: None,
        blocked_reason: None,
        status_updated_at: None,
        parent_session_id: None,
        branch_of_turn_id: None,
        subagent: None,
        revision_root_session_id: None,
        revision_parent_session_id: None,
        revision_of_turn_id: None,
        revision_index: None,
        revision_state: None,
        last_read_message_id: record.read_state.last_read_message_id,
        live_run_state: None,
    }
}
