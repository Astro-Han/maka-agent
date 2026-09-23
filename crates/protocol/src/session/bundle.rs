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

//! Native history transfer names paths on the Host, never on a remote client.
use super::WorkspaceTarget;
use crate::{OperationErrorCode, ProtocolError, Result, codec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preview {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Previewed {
    pub session_count: u64,
    pub subtree_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Export {
    pub session_id: String,
    pub destination: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_subtree_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Exported {
    pub session_count: u64,
    pub compressed_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Import {
    pub source: String,
    pub workspace: WorkspaceTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Imported {
    pub session_count: u64,
    pub artifact_files: u64,
}

pub fn decode_preview(value: &Value) -> Result<Preview> {
    super::validation::decode(value)
}

pub fn decode_previewed(value: &Value) -> Result<Previewed> {
    let output: Previewed = decode(value)?;
    counts(value, &["sessionCount"])?;
    digest(&output.subtree_digest)?;
    Ok(output)
}

pub fn decode_export(value: &Value) -> Result<Export> {
    let input: Export = super::validation::decode(value)?;
    path(&input.destination)?;
    if let Some(expected) = &input.expected_subtree_digest {
        digest(expected)?;
    }
    Ok(input)
}

fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ProtocolError::invalid("Invalid subtree digest"));
    }
    Ok(())
}

pub fn decode_import(value: &Value) -> Result<Import> {
    let input: Import = super::validation::decode(value)?;
    path(&input.source)?;
    Ok(input)
}

pub fn decode_exported(value: &Value) -> Result<Exported> {
    let output: Exported = decode(value)?;
    counts(value, &["sessionCount", "compressedBytes"])?;
    Ok(output)
}

pub fn decode_imported(value: &Value) -> Result<Imported> {
    let output: Imported = decode(value)?;
    counts(value, &["sessionCount", "artifactFiles"])?;
    Ok(output)
}

fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|e| ProtocolError::invalid(e.to_string()))
}

fn path(path: &str) -> Result<()> {
    if path.len() > 4096 || path.contains('\0') || !codec::absolute_host_path(path) {
        return Err(ProtocolError::invalid(
            "Bundle path must be an absolute Host path",
        ));
    }
    Ok(())
}

fn counts(value: &Value, names: &[&str]) -> Result<()> {
    for name in names {
        codec::count(&value[*name], name)?;
    }
    Ok(())
}

pub const ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::NotFound,
    OperationErrorCode::SessionBusy,
    OperationErrorCode::OperationConflict,
    OperationErrorCode::SourceUnreadable,
    OperationErrorCode::CandidateSetStale,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::CommitOutcomeUnknown,
    OperationErrorCode::InternalFailure,
];
