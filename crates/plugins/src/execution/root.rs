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

use super::{Error, SessionBoundary, name};
use maka_runtime::execution::{
    BehaviorId, CollaborationMode, ModelBinding, PermissionMode, ThinkingLevel, ToolMode,
    WorkspaceIdentity, WorkspaceTarget,
};
use serde::{Deserialize, Serialize};

/// Explicit choices for a new root Session. No mutable Host defaults are
/// resolved behind a retry identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub target: super::Target,
    pub permission_mode: PermissionMode,
    pub tool_mode: ToolMode,
    pub collaboration_mode: CollaborationMode,
    pub behavior: BehaviorId,
    #[serde(default)]
    pub bound_tools: Option<std::collections::BTreeSet<String>>,
    #[serde(default)]
    pub instructions: Option<String>,
}
impl Settings {
    pub fn validate(&self) -> Result<(), Error> {
        self.target.validate()?;
        if self
            .instructions
            .as_ref()
            .is_some_and(|text| text.len() > 16 * 1024)
            || self
                .bound_tools
                .as_ref()
                .is_some_and(|tools| tools.len() > 128)
        {
            return Err(Error::Invalid(
                "Session settings exceed their budget".into(),
            ));
        }
        if let Some(tools) = &self.bound_tools {
            for tool in tools {
                name(tool)?;
            }
        }
        Ok(())
    }
}

/// Fully explicit execution surface approved by Host. Defaults are resolved
/// before persistence; a later restart cannot reinterpret configuration defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootTemplate {
    pub workspace: WorkspaceTarget,
    pub cwd: String,
    pub workspace_identity: WorkspaceIdentity,
    pub model: ModelBinding,
    pub thinking_level: Option<ThinkingLevel>,
    pub tool_mode: ToolMode,
    pub permission_mode: PermissionMode,
    pub collaboration_mode: CollaborationMode,
    pub orchestration_mode: BehaviorId,
}
impl RootTemplate {
    pub fn settings(&self) -> Settings {
        Settings {
            target: super::Target::Model {
                model: self.model.clone(),
                thinking_level: self.thinking_level,
            },
            permission_mode: self.permission_mode,
            tool_mode: self.tool_mode,
            collaboration_mode: self.collaboration_mode,
            behavior: self.orchestration_mode.clone(),
            bound_tools: None,
            instructions: None,
        }
    }
    pub fn validate(&self) -> Result<(), Error> {
        name(&self.model.connection_id)?;
        name(&self.model.connection_slug)?;
        name(&self.model.model)?;
        if self.cwd.is_empty() || self.cwd.len() > 32 * 1024 || self.cwd.contains('\0') {
            return Err(Error::Invalid("invalid root Session workspace".into()));
        }
        match &self.workspace {
            WorkspaceTarget::Project { project_id } => name(project_id)?,
            WorkspaceTarget::HostPath { path } if path == &self.cwd => {}
            _ => {
                return Err(Error::Invalid(
                    "root Session workspace must be canonical".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Frozen Host approval data. Restoring a capability requires an explicit Host
/// grant; normal execution capabilities cannot create root Sessions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootApproval {
    pub template: RootTemplate,
    /// Agent-created plans retain their originating Session's permission ceiling.
    pub source: Option<SessionBoundary>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateRoot {
    pub operation_id: String,
    pub name: String,
    pub settings: Settings,
}
impl CreateRoot {
    pub fn validate(&self) -> Result<(), Error> {
        name(&self.operation_id)?;
        self.settings.validate()?;
        if self.name.trim().is_empty() || self.name.len() > 1024 || self.name.contains('\0') {
            return Err(Error::Invalid("invalid root Session name".into()));
        }
        Ok(())
    }
}
