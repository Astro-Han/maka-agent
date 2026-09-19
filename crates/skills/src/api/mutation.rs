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

use super::{GovernanceItem, WorkspaceContext};
use maka_runtime::execution::{WorkspaceProjection, WorkspaceTarget};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Mutation {
    CreateStarter,
    Install {
        source_type: InstallSource,
        source_id: String,
    },
    UpdateManaged(super::ManagedUpdate),
    Delete {
        #[serde(rename = "ref")]
        reference: String,
    },
    SetEnabled {
        #[serde(rename = "ref")]
        reference: String,
        enabled: bool,
    },
    SetPinned {
        #[serde(rename = "ref")]
        reference: String,
        pinned: bool,
    },
}
impl Mutation {
    pub fn reference(&self) -> Option<&str> {
        match self {
            Self::SetEnabled { reference, .. }
            | Self::SetPinned { reference, .. }
            | Self::Delete { reference } => Some(reference),
            Self::UpdateManaged(update) => Some(&update.reference),
            Self::CreateStarter | Self::Install { .. } => None,
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallSource {
    Bundled,
    Managed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutateInput {
    pub context: WorkspaceContext,
    pub expected_revision: String,
    pub mutation: Mutation,
}
impl MutateInput {
    pub fn uses_host_paths(&self) -> bool {
        matches!(self.context.workspace, WorkspaceTarget::HostPath { .. })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationRejection {
    NotFound,
    AlreadyExists,
    BlockedScope,
    NotManaged,
    SourceMissing,
    SourceChanged,
    SourceInvalid,
    LocalModified,
    MetadataError,
    BlockedPath,
    NeedsReview,
    StateError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum MutationOutcome {
    Committed {
        revision: String,
        entry: Option<MutationEntry>,
    },
    Unchanged {
        revision: String,
        entry: Option<MutationEntry>,
    },
    RevisionConflict {
        expected_revision: String,
        actual_revision: String,
    },
    Rejected {
        reason: MutationRejection,
    },
}
/// Mutation replies preserve the same tagged Skill shape as catalog rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MutationEntry {
    Skill(GovernanceItem),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationResult {
    #[serde(flatten)]
    pub outcome: MutationOutcome,
    pub resolved_workspace: WorkspaceProjection,
}
