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

use super::WorkspaceContext;
use maka_runtime::execution::{WorkspaceProjection, WorkspaceTarget};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogView {
    Governance,
    Bundled,
    ManagedSources,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CatalogInput {
    Start {
        context: WorkspaceContext,
        view: CatalogView,
    },
    Continue {
        context: WorkspaceContext,
        view: CatalogView,
        revision: String,
        cursor: String,
    },
}
impl CatalogInput {
    pub fn context(&self) -> &WorkspaceContext {
        match self {
            Self::Start { context, .. } | Self::Continue { context, .. } => context,
        }
    }
    pub fn view(&self) -> CatalogView {
        match self {
            Self::Start { view, .. } | Self::Continue { view, .. } => *view,
        }
    }
    pub fn uses_host_paths(&self) -> bool {
        matches!(self.context().workspace, WorkspaceTarget::HostPath { .. })
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedSourceType {
    Local,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CatalogItem {
    Skill(super::GovernanceItem),
    DiscoveryDiagnostic(super::GovernanceItem),
    Bundled {
        id: String,
        name: String,
        description: String,
        category: String,
        declared_tools: Vec<String>,
        metadata_truncated: bool,
        installed: bool,
    },
    ManagedSource {
        id: String,
        name: String,
        description: String,
        category: String,
        source_type: ManagedSourceType,
        metadata_truncated: bool,
        installed: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CatalogResult {
    Page {
        user_recovery: Option<String>,
        view: CatalogView,
        revision: String,
        items: Vec<CatalogItem>,
        next_cursor: Option<String>,
        resolved_workspace: WorkspaceProjection,
    },
    RevisionChanged {
        expected_revision: String,
        actual_revision: String,
        resolved_workspace: WorkspaceProjection,
    },
}
