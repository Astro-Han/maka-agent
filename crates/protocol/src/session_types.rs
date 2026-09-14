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

pub use maka_runtime::execution::{
    CollaborationMode, OrchestrationMode, PermissionMode, ThinkingLevel, WorkspaceTarget,
};
use serde::{Deserialize, Serialize};

macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $wire)] $variant),+ }
    };
}
wire_enum!(SessionStartMode { DeepResearch=>"deep_research", Bot=>"bot" });
wire_enum!(SessionToolProfile { HeadlessCodingV1=>"headless-coding-v1", WorkhubCoordinationV1=>"workhub-coordination-v1", WorkhubCoordinationV2=>"workhub-coordination-v2" });
wire_enum!(SessionStatus { Active=>"active", Running=>"running", WaitingForUser=>"waiting_for_user", Blocked=>"blocked", Aborted=>"aborted" });
wire_enum!(BlockedReason { NoRealConnection=>"NO_REAL_CONNECTION", Auth=>"auth", PermissionRequired=>"permission_required", ToolFailed=>"tool_failed", Unknown=>"unknown" });
wire_enum!(Backend { AiSdk=>"ai-sdk", Fake=>"fake" });
wire_enum!(RevisionState { Preparing=>"preparing", Committed=>"committed" });
wire_enum!(SessionLifecycleState { Active=>"active", Archived=>"archived" });
wire_enum!(UnsupportedReason { NotWireRepresentable=>"not_wire_representable" });

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceProjection {
    pub target: WorkspaceTarget,
    pub host_cwd: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SessionModelTarget {
    Default,
    Explicit {
        connection_id: String,
        connection_slug: String,
        model: String,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCatalogLiveRunState {
    pub schema_version: u64,
    pub running_turn_ids: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSubagentProjection {
    pub parent_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCatalogProjection {
    pub id: String,
    pub revision: u64,
    pub workspace: WorkspaceProjection,
    pub created_at: u64,
    pub activity_at: u64,
    pub name: String,
    pub is_flagged: bool,
    pub is_archived: bool,
    pub labels: Vec<String>,
    pub labels_truncated: bool,
    pub has_unread: bool,
    pub status: SessionStatus,
    pub backend: Backend,
    pub llm_connection_id: Option<String>,
    pub llm_connection_slug: String,
    pub connection_locked: bool,
    pub model: String,
    pub permission_mode: PermissionMode,
    pub collaboration_mode: CollaborationMode,
    pub orchestration_mode: OrchestrationMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<BlockedReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_updated_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_of_turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent: Option<SessionSubagentProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_root_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_of_turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_index: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_state: Option<RevisionState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_read_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_run_state: Option<SessionCatalogLiveRunState>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UnsupportedLegacyRecord {
    UnsupportedLegacyRecord {
        id: String,
        revision: u64,
        reason: UnsupportedReason,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionCatalogItem {
    Projection(Box<SessionCatalogProjection>),
    UnsupportedLegacy(UnsupportedLegacyRecord),
}
impl SessionCatalogItem {
    pub fn id(&self) -> &str {
        match self {
            Self::Projection(p) => &p.id,
            Self::UnsupportedLegacy(UnsupportedLegacyRecord::UnsupportedLegacyRecord {
                id,
                ..
            }) => id,
        }
    }
}
