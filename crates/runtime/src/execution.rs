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

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Explore,
    Ask,
    Bypass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationMode {
    Agent,
    Plan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationMode {
    Default,
    Swarm,
    Graph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMode {
    #[default]
    Direct,
    CodeMode,
}

/// Public model identity. Credentials and provider handles never belong here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelBinding {
    pub connection_id: String,
    pub connection_slug: String,
    pub model: String,
}

/// Actual Host-composed instructions admitted with this invocation, not a
/// pointer to mutable root settings or a replacement for conversation history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPrompt {
    pub text: String,
    pub policy_revision: u64,
}

impl SystemPrompt {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.text.trim().is_empty()
            || self.text.len() > 64 * 1024
            || self.policy_revision > crate::configuration::validation::MAX_SAFE_INTEGER
        {
            return Err("invalid bounded system prompt");
        }
        Ok(())
    }
}

/// Immutable admission context, committed in the opening before dispatch.
/// Later Session mutations must not reinterpret relative paths or grants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvocationConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_composition: Option<ToolComposition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<SystemPrompt>,
    pub cwd: String,
    /// Captured from the workspace marker, never reconstructed from a later path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<WorkspaceIdentity>,
    pub permission_mode: PermissionMode,
    pub collaboration_mode: CollaborationMode,
    pub orchestration_mode: OrchestrationMode,
    pub tool_mode: ToolMode,
    /// None means there was no Session-owned model binding (e.g. local Code).
    pub model: Option<ModelBinding>,
    pub thinking_level: Option<ThinkingLevel>,
}

/// Admission evidence for Host-owned handlers whose behavior is not in schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolComposition {
    pub clients: crate::capability::ClientComposition,
    /// None for fixed profiles without Skill / SkillSearch handlers.
    pub skills_digest: Option<String>,
}

/// User-selected workspace locator, before Host path resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkspaceTarget {
    Project { project_id: String },
    HostPath { path: String },
}

/// Intrinsic workspace identity is distinct from its current filesystem location.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorkspaceIdentity(String);

impl WorkspaceIdentity {
    pub fn from_marker_id(id: &str) -> Result<Self, &'static str> {
        let uuid = uuid::Uuid::parse_str(id).map_err(|_| "invalid workspace UUID")?;
        if id.len() != 36
            || !uuid.hyphenated().to_string().eq_ignore_ascii_case(id)
            || !(1..=8).contains(&uuid.get_version_num())
            || uuid.get_variant() != uuid::Variant::RFC4122
        {
            return Err("invalid workspace UUID");
        }
        // Preserve an existing marker's exact spelling, including uppercase UUIDs.
        Ok(Self(format!("workspace:v1:{id}")))
    }

    pub fn marker_id(&self) -> &str {
        &self.0["workspace:v1:".len()..]
    }
}

impl TryFrom<String> for WorkspaceIdentity {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_marker_id(
            value
                .strip_prefix("workspace:v1:")
                .ok_or("invalid workspace identity")?,
        )
    }
}

impl From<WorkspaceIdentity> for String {
    fn from(identity: WorkspaceIdentity) -> Self {
        identity.0
    }
}
