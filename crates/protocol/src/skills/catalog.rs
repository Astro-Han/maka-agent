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

use super::{MAX_ITEMS, MAX_PAGE_BYTES, WorkspaceContext, invalid, revision, text};
use crate::{Result, codec, session::WorkspaceProjection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
        matches!(
            self.context().workspace,
            crate::session::WorkspaceTarget::HostPath { .. }
        )
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
pub fn decode_catalog_input(value: &Value) -> Result<CatalogInput> {
    let input: CatalogInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    super::validate_workspace(&input.context().workspace)?;
    if let CatalogInput::Continue {
        revision: r,
        cursor,
        ..
    } = &input
    {
        revision(r)?;
        text(cursor, 1024)?;
    }
    Ok(input)
}
pub fn decode_catalog_output(value: &Value) -> Result<CatalogResult> {
    if value["kind"] == "page" {
        codec::exact(
            codec::record(value, "Skill source page")?,
            &[
                "kind",
                "view",
                "revision",
                "items",
                "nextCursor",
                "resolvedWorkspace",
            ],
        )?;
    }
    let result: CatalogResult = serde_json::from_value(value.clone()).map_err(invalid)?;
    let workspace = match &result {
        CatalogResult::Page {
            view,
            revision: r,
            items,
            next_cursor,
            resolved_workspace,
        } => {
            revision(r)?;
            if items.len() > MAX_ITEMS {
                return Err(invalid("Too many skill source items"));
            }
            let workspace_overhead = ",\"resolvedWorkspace\":".len()
                + serde_json::to_vec(resolved_workspace)
                    .map_err(invalid)?
                    .len();
            if serde_json::to_vec(&result).map_err(invalid)?.len()
                > MAX_PAGE_BYTES + workspace_overhead
            {
                return Err(invalid("Skill source page exceeds byte limit"));
            }
            if let Some(cursor) = next_cursor {
                text(cursor, 1024)?;
            }
            for item in items {
                let (id, name, description, category) = match item {
                    CatalogItem::Skill(item) | CatalogItem::DiscoveryDiagnostic(item) => {
                        if *view != CatalogView::Governance {
                            return Err(invalid("Invalid governance page"));
                        }
                        item.validate()?;
                        continue;
                    }
                    CatalogItem::Bundled {
                        id,
                        name,
                        description,
                        category,
                        declared_tools,
                        ..
                    } => {
                        if *view != CatalogView::Bundled || declared_tools.len() > 64 {
                            return Err(invalid("Invalid bundled page"));
                        }
                        for tool in declared_tools {
                            text(tool, 256)?;
                        }
                        (id, name, description, category)
                    }
                    CatalogItem::ManagedSource {
                        id,
                        name,
                        description,
                        category,
                        ..
                    } => {
                        if *view != CatalogView::ManagedSources {
                            return Err(invalid("Invalid managed source page"));
                        }
                        (id, name, description, category)
                    }
                };
                text(id, 81)?;
                if !id.as_bytes()[0].is_ascii_alphanumeric()
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                {
                    return Err(invalid("Invalid source id"));
                }
                text(name, 256)?;
                text(category, 128)?;
                if description.len() > 4096 {
                    return Err(invalid("Invalid source description"));
                }
            }
            resolved_workspace
        }
        CatalogResult::RevisionChanged {
            expected_revision,
            actual_revision,
            resolved_workspace,
        } => {
            revision(expected_revision)?;
            revision(actual_revision)?;
            resolved_workspace
        }
    };
    super::validate_workspace(&workspace.target)?;
    super::validate_workspace(&crate::session::WorkspaceTarget::HostPath {
        path: workspace.host_cwd.clone(),
    })?;
    if let crate::session::WorkspaceTarget::HostPath { path } = &workspace.target
        && path != &workspace.host_cwd
    {
        return Err(invalid("Workspace path mismatch"));
    }
    Ok(result)
}
