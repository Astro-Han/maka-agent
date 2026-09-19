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

pub const MAX_PREVIEW_BYTES: usize = 48 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewInput {
    pub context: WorkspaceContext,
    pub expected_revision: String,
    #[serde(rename = "ref")]
    pub reference: String,
}
impl PreviewInput {
    pub fn uses_host_paths(&self) -> bool {
        matches!(self.context.workspace, WorkspaceTarget::HostPath { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewRejection {
    NotFound,
    NotManaged,
    SourceMissing,
    SourceInvalid,
    MetadataError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LineSummary {
    pub current_line_count: usize,
    pub source_line_count: usize,
    pub changed_line_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PreviewOutcome {
    Preview {
        revision: String,
        current_snippet: String,
        source_snippet: String,
        current_truncated: bool,
        source_truncated: bool,
        has_managed_baseline: bool,
        summary: LineSummary,
        expected_current_sha256: String,
        expected_source_sha256: String,
    },
    RevisionConflict {
        expected_revision: String,
        actual_revision: String,
    },
    Rejected {
        reason: PreviewRejection,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResult {
    #[serde(flatten)]
    pub outcome: PreviewOutcome,
    pub resolved_workspace: WorkspaceProjection,
}
