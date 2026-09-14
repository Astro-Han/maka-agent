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

use super::{SessionCatalogItem, identity, validation};
use crate::{ProtocolError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMetadataPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_flagged: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMetadataUpdateInput {
    pub session_id: String,
    pub expected_revision: u64,
    pub patch: SessionMetadataPatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SessionUpdateResult {
    Committed {
        session: SessionCatalogItem,
    },
    RevisionConflict {
        expected_revision: u64,
        actual_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionReadMarkerSetInput {
    pub session_id: String,
    pub read_through_message_id: String,
}

pub fn decode_session_metadata_update_input(value: &Value) -> Result<SessionMetadataUpdateInput> {
    let input: SessionMetadataUpdateInput = validation::decode(value)?;
    if input.patch.name.is_none()
        && input.patch.labels.is_none()
        && input.patch.is_flagged.is_none()
    {
        return Err(ProtocolError::invalid("Session metadata patch is empty"));
    }
    Ok(input)
}

pub fn decode_session_update_result(value: &Value) -> Result<SessionUpdateResult> {
    validation::decode(value)
}

pub fn decode_session_read_marker_set_input(value: &Value) -> Result<SessionReadMarkerSetInput> {
    validation::decode(value)
}

pub fn assert_metadata_update_output_for_input(
    input: &SessionMetadataUpdateInput,
    output: &SessionUpdateResult,
) -> Result<()> {
    assert_update_output(&input.session_id, input.expected_revision, output)
}

pub(super) fn assert_update_output(
    session_id: &str,
    revision: u64,
    output: &SessionUpdateResult,
) -> Result<()> {
    match output {
        SessionUpdateResult::Committed { session } => identity(session_id, session.id()),
        SessionUpdateResult::RevisionConflict {
            expected_revision, ..
        } if *expected_revision != revision => Err(ProtocolError::invalid(
            "Session revision conflict does not match request",
        )),
        _ => Ok(()),
    }
}

pub fn assert_read_marker_output_for_input(
    input: &SessionReadMarkerSetInput,
    output: &SessionCatalogItem,
) -> Result<()> {
    identity(&input.session_id, output.id())
}
