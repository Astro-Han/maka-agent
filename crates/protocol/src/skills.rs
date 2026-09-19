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

use crate::{ProtocolError, Result, codec, session::WorkspaceTarget};
use serde_json::Value;

pub use maka_skills::api::*;
mod catalog;
mod governance;
mod mutation;
pub use mutation::{decode_mutate_input, decode_mutate_output};
mod import;
mod path;
pub use import::{decode_import_input, decode_import_output};
mod preview;
pub use catalog::{decode_catalog_input, decode_catalog_output};
pub use path::{decode_path_input, decode_path_output};
pub use preview::{decode_preview_input, decode_preview_output};

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
fn validate_projection(workspace: &maka_runtime::execution::WorkspaceProjection) -> Result<()> {
    validate_workspace(&workspace.target)?;
    validate_workspace(&WorkspaceTarget::HostPath {
        path: workspace.host_cwd.clone(),
    })?;
    if let WorkspaceTarget::HostPath { path } = &workspace.target
        && path != &workspace.host_cwd
    {
        return Err(invalid("Workspace path mismatch"));
    }
    Ok(())
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
