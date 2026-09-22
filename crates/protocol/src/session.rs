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

//! Session catalog and retirement wire contracts.
//! Use the decode functions at the JSON boundary; they enforce semantic limits
//! in addition to the owned serde representations.
mod configuration;
mod mutation;
mod types;
mod validation;
use crate::{ProtocolError, Result};
pub use configuration::*;
pub use mutation::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use types::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreateInput {
    pub session_id: String,
    pub workspace: WorkspaceTarget,
    #[serde(flatten)]
    pub target: SessionCreateTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<SessionStartMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_profile: Option<SessionToolProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<SandboxMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<CollaborationMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestration_mode: Option<BehaviorId>,
}

/// The untagged wire contract selects exactly one backend. The flattened
/// target rejects leftover fields, including a second target or unknown keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum SessionCreateTarget {
    Model {
        model_target: SessionModelTarget,
    },
    Executor {
        executor_id: maka_runtime::executor::ExecutorId,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SessionCatalogQueryInput {
    ListStart,
    ListContinue { revision: String, cursor: String },
    Get { session_id: String },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SessionCatalogQueryResult {
    Page {
        revision: String,
        sessions: Vec<SessionCatalogProjection>,
        next_cursor: Option<String>,
    },
    RevisionChanged {
        expected_revision: String,
        actual_revision: String,
    },
    Session {
        session: Option<Box<SessionCatalogProjection>>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionLifecycleSetInput {
    pub session_id: String,
    pub state: SessionLifecycleState,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRemoveInput {
    pub session_id: String,
    pub expected_revision: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRemovePreviewInput {
    pub session_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRemovePreviewResult {
    pub archivable_subtask_count: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SessionRemoveResult {
    Removed {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        archived_subtask_count: Option<u64>,
    },
    RevisionConflict {
        expected_revision: u64,
        actual_revision: u64,
    },
}

macro_rules! decoder {
    ($name:ident, $type:ty) => {
        pub fn $name(value: &Value) -> Result<$type> {
            validation::decode(value)
        }
    };
}
decoder!(decode_session_create_input, SessionCreateInput);
decoder!(decode_session_catalog_query_input, SessionCatalogQueryInput);
decoder!(decode_session_catalog_projection, SessionCatalogProjection);
decoder!(
    decode_session_catalog_query_result,
    SessionCatalogQueryResult
);
decoder!(decode_session_lifecycle_set_input, SessionLifecycleSetInput);
decoder!(decode_session_remove_input, SessionRemoveInput);
decoder!(decode_session_remove_result, SessionRemoveResult);
decoder!(
    decode_session_remove_preview_input,
    SessionRemovePreviewInput
);
decoder!(
    decode_session_remove_preview_result,
    SessionRemovePreviewResult
);

pub fn assert_create_output_for_input(
    input: &SessionCreateInput,
    output: &SessionCatalogProjection,
) -> Result<()> {
    identity(&input.session_id, &output.id)
}
pub fn assert_lifecycle_output_for_input(
    input: &SessionLifecycleSetInput,
    output: &SessionCatalogProjection,
) -> Result<()> {
    identity(&input.session_id, &output.id)?;
    if output.is_archived != (input.state == SessionLifecycleState::Archived) {
        return Err(ProtocolError::invalid(
            "Session lifecycle state does not match request",
        ));
    }
    Ok(())
}
pub fn assert_remove_output_for_input(
    input: &SessionRemoveInput,
    output: &SessionRemoveResult,
) -> Result<()> {
    match output {
        SessionRemoveResult::Removed { session_id, .. } => identity(&input.session_id, session_id),
        SessionRemoveResult::RevisionConflict {
            expected_revision, ..
        } if *expected_revision != input.expected_revision => Err(ProtocolError::invalid(
            "Session remove conflict changed expected revision",
        )),
        _ => Ok(()),
    }
}
fn identity(expected: &str, actual: &str) -> Result<()> {
    if expected != actual {
        return Err(ProtocolError::invalid(
            "Session result identity does not match request",
        ));
    }
    Ok(())
}
