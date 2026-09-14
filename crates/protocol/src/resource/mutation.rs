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

use super::{MAX_RESULT_BYTES, entity, invalid, text};
use crate::{OperationErrorCode as Code, Result};
use maka_presentation::shell::ShellSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MUTATION_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::NotFound,
    Code::SessionArchived,
    Code::OperationConflict,
    Code::InvalidRequest,
    Code::InternalFailure,
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceStartInput {
    pub session_id: String,
    pub launch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceStopInput {
    pub session_id: String,
    #[serde(rename = "ref")]
    pub resource_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceMutationResult {
    pub resource: ShellSnapshot,
}

pub fn decode_start_input(value: &Value) -> Result<ResourceStartInput> {
    let input: ResourceStartInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    entity(&input.session_id)?;
    crate::codec::string(&value["launchId"], "launchId", 128)?;
    if value.get("command").is_some_and(Value::is_null)
        || input
            .command
            .as_ref()
            .is_some_and(|s| s.trim().is_empty() || s.len() > 32 * 1024)
    {
        return Err(invalid("invalid Runtime Resource command"));
    }
    Ok(input)
}

pub fn decode_stop_input(value: &Value) -> Result<ResourceStopInput> {
    let input: ResourceStopInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    entity(&input.session_id)?;
    text(&input.resource_ref, 256)?;
    Ok(input)
}

pub fn decode_mutation_result(value: &Value) -> Result<ResourceMutationResult> {
    let output: ResourceMutationResult = serde_json::from_value(value.clone()).map_err(invalid)?;
    output.resource.validate().map_err(invalid)?;
    if serde_json::to_vec(&output).map_err(invalid)?.len() > MAX_RESULT_BYTES {
        return Err(invalid("resource result exceeds byte limit"));
    }
    Ok(output)
}
