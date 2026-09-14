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

use crate::{
    ProtocolError, Result, codec,
    session::{CollaborationMode, PermissionMode, WorkspaceTarget},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_PAGE_BYTES: usize = 48 * 1024;
pub const MAX_ITEMS: usize = 128;
mod catalog;
pub use catalog::*;
mod governance;
pub use governance::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceContext {
    pub workspace: WorkspaceTarget,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum InvocableTarget {
    Session {
        session_id: String,
    },
    NewSession {
        context: WorkspaceContext,
        collaboration_mode: CollaborationMode,
        permission_mode: PermissionMode,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum InvocableInput {
    Start {
        target: InvocableTarget,
    },
    Continue {
        target: InvocableTarget,
        revision: String,
        cursor: String,
    },
}
impl InvocableInput {
    pub fn target(&self) -> &InvocableTarget {
        match self {
            Self::Start { target } | Self::Continue { target, .. } => target,
        }
    }
    pub fn uses_host_paths(&self) -> bool {
        matches!(
            self.target(),
            InvocableTarget::NewSession {
                context: WorkspaceContext {
                    workspace: WorkspaceTarget::HostPath { .. }
                },
                ..
            }
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocableItem {
    #[serde(rename = "ref")]
    pub reference: String,
    pub id: String,
    pub name: String,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum InvocableResult {
    Page {
        revision: String,
        items: Vec<InvocableItem>,
        next_cursor: Option<String>,
    },
    RevisionChanged {
        expected_revision: String,
        actual_revision: String,
    },
}

pub fn decode_invocable_input(value: &Value) -> Result<InvocableInput> {
    let input: InvocableInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    match input.target() {
        InvocableTarget::Session { session_id } => crate::turn::entity(session_id)?,
        InvocableTarget::NewSession { context, .. } => validate_workspace(&context.workspace)?,
    }
    if let InvocableInput::Continue {
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
pub fn decode_invocable_output(value: &Value) -> Result<InvocableResult> {
    if value["kind"] == "page" {
        codec::exact(
            codec::record(value, "Skill page")?,
            &["kind", "revision", "items", "nextCursor"],
        )?;
    }
    let result: InvocableResult = serde_json::from_value(value.clone()).map_err(invalid)?;
    match &result {
        InvocableResult::Page {
            revision: r,
            items,
            next_cursor,
        } => {
            revision(r)?;
            if items.len() > MAX_ITEMS
                || serde_json::to_vec(&result).map_err(invalid)?.len() > MAX_PAGE_BYTES
            {
                return Err(invalid("Skill page exceeds limits"));
            }
            for item in items {
                text(&item.reference, 512)?;
                text(&item.id, 256)?;
                text(&item.name, 256)?;
                text(&item.description, 4096)?;
            }
            if let Some(cursor) = next_cursor {
                text(cursor, 1024)?;
            }
        }
        InvocableResult::RevisionChanged {
            expected_revision,
            actual_revision,
        } => {
            revision(expected_revision)?;
            revision(actual_revision)?;
        }
    }
    Ok(result)
}
fn text(value: &str, maximum: usize) -> Result<()> {
    if value.is_empty() || value.len() > maximum {
        Err(invalid("Invalid Skill catalog string"))
    } else {
        Ok(())
    }
}
fn validate_workspace(workspace: &WorkspaceTarget) -> Result<()> {
    match workspace {
        WorkspaceTarget::Project { project_id } => crate::turn::entity(project_id),
        WorkspaceTarget::HostPath { path } => {
            text(path, 4096)?;
            if codec::absolute_host_path(path) {
                Ok(())
            } else {
                Err(invalid("Invalid workspace path"))
            }
        }
    }
}
fn revision(value: &str) -> Result<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(invalid("Invalid Skill revision"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("Invalid Skill revision"));
    }
    Ok(())
}
fn invalid(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::invalid(error.to_string())
}
