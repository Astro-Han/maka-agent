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

use super::{
    CollaborationMode, OrchestrationMode, PermissionMode, SessionModelTarget, SessionUpdateResult,
    ThinkingLevel, WorkspaceTarget, mutation, validation,
};
use crate::{ProtocolError, Result};
use maka_runtime::configuration::Patch;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionConfigurationPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_target: Option<SessionModelTarget>,
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub thinking_level: Patch<ThinkingLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<CollaborationMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orchestration_mode: Option<OrchestrationMode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionConfigurationUpdateInput {
    pub session_id: String,
    pub expected_revision: u64,
    pub patch: SessionConfigurationPatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionWorkspaceRelocateInput {
    pub session_id: String,
    pub expected_revision: u64,
    pub workspace: WorkspaceTarget,
}

pub fn decode_session_workspace_relocate_input(
    value: &Value,
) -> Result<SessionWorkspaceRelocateInput> {
    validation::decode(value)
}

pub fn assert_workspace_relocate_output_for_input(
    input: &SessionWorkspaceRelocateInput,
    output: &SessionUpdateResult,
) -> Result<()> {
    mutation::assert_update_output(&input.session_id, input.expected_revision, output)
}

pub fn decode_session_configuration_update_input(
    value: &Value,
) -> Result<SessionConfigurationUpdateInput> {
    // Only this patch permits null thinking. Keep the shared Session validator
    // strict for create inputs, projections, and all other patch fields.
    let mut normalized = value.clone();
    let clear_thinking = normalized
        .get_mut("patch")
        .and_then(Value::as_object_mut)
        .is_some_and(|patch| {
            if patch.get("thinkingLevel").is_some_and(Value::is_null) {
                patch.remove("thinkingLevel");
                true
            } else {
                false
            }
        });
    let mut input: SessionConfigurationUpdateInput = validation::decode(&normalized)?;
    if clear_thinking {
        input.patch.thinking_level = Patch::Clear;
    }
    let patch = &input.patch;
    if patch.model_target.is_none()
        && patch.thinking_level.is_keep()
        && patch.permission_mode.is_none()
        && patch.collaboration_mode.is_none()
        && patch.orchestration_mode.is_none()
    {
        return Err(ProtocolError::invalid(
            "Session configuration patch is empty",
        ));
    }
    if matches!(patch.model_target, Some(SessionModelTarget::Default)) {
        return Err(ProtocolError::invalid(
            "Session configuration model target must be explicit",
        ));
    }
    Ok(input)
}

pub fn assert_configuration_update_output_for_input(
    input: &SessionConfigurationUpdateInput,
    output: &SessionUpdateResult,
) -> Result<()> {
    mutation::assert_update_output(&input.session_id, input.expected_revision, output)
}
