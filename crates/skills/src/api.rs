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

//! Skills domain contracts shared by native adapters and plugin consumers.
use maka_runtime::execution::{CollaborationMode, SandboxMode, WorkspaceTarget};
use serde::{Deserialize, Serialize};

pub const MAX_PAGE_BYTES: usize = 48 * 1024;
pub const MAX_ITEMS: usize = 128;
mod catalog;
mod governance;
mod import;
pub use import::*;
mod mutation;
mod path;
mod preview;
pub use path::*;
mod update;
pub use catalog::*;
pub use governance::*;
pub use mutation::*;
pub use preview::*;
pub use update::*;

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
        sandbox_mode: SandboxMode,
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
