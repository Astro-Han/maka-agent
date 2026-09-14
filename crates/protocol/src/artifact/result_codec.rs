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

use super::*;
use maka_runtime::{artifact::Artifact, attachment::StorageRef};

pub fn decode_ingest_result(value: &Value) -> Result<ArtifactIngestResult> {
    let result = decode_result(value)?;
    match &result {
        ArtifactIngestResult::UploadOpened { upload_id, .. }
        | ArtifactIngestResult::ChunkAccepted { upload_id, .. }
        | ArtifactIngestResult::UploadAborted { upload_id } => entity(upload_id)?,
        ArtifactIngestResult::Committed {
            upload_id,
            attachment,
        } => {
            entity(upload_id)?;
            ensure(
                !attachment.name.is_empty() && !attachment.mime_type.is_empty(),
                "Invalid committed attachment metadata",
            )?;
            let StorageRef::SessionFile {
                session_id,
                relative_path,
            } = &attachment.storage_ref
            else {
                return Err(ProtocolError::invalid(
                    "Invalid committed attachment reference",
                ));
            };
            entity(session_id)?;
            ensure(
                maka_runtime::attachment::canonical_relative_path(relative_path),
                "Invalid committed attachment path",
            )?;
        }
    }
    Ok(result)
}

pub fn decode_query_result(value: &Value) -> Result<ArtifactQueryResult> {
    let result = decode_result(value)?;
    match &result {
        ArtifactQueryResult::RevisionChanged { expected, actual } => {
            digest(expected)?;
            digest(actual)?;
        }
        ArtifactQueryResult::Artifact {
            session_id,
            revision,
            artifact,
        } => {
            entity(session_id)?;
            digest(revision)?;
            if let Some(artifact) = artifact {
                validate_artifact(artifact)?;
            }
        }
        ArtifactQueryResult::Page {
            session_id,
            revision,
            artifacts,
            next_cursor,
        } => {
            entity(session_id)?;
            digest(revision)?;
            ensure(
                artifacts.len() <= MAX_PAGE_ITEMS,
                "Artifact page exceeds item limit",
            )?;
            for artifact in artifacts {
                validate_artifact(artifact)?;
            }
            if let Some(cursor) = next_cursor {
                text(cursor, 32, false)?;
            }
        }
        ArtifactQueryResult::Text {
            session_id,
            artifact_id,
            preview,
        } => {
            entity(session_id)?;
            entity(artifact_id)?;
            if let Ok(preview) = preview {
                text(&preview.text, MAX_PREVIEW_BYTES, true)?;
            }
        }
        ArtifactQueryResult::Binary {
            session_id,
            artifact_id,
            preview,
        } => {
            entity(session_id)?;
            entity(artifact_id)?;
            if let Ok(preview) = preview {
                text(&preview.mime_type, 512, false)?;
                text(&preview.base64, MAX_PREVIEW_BYTES.div_ceil(3) * 4, true)?;
                decode_chunk(&preview.base64, MAX_PREVIEW_BYTES)?;
            }
        }
        ArtifactQueryResult::Chunk {
            session_id,
            artifact_id,
            offset,
            total_bytes,
            chunk_base64,
            next_offset,
        } => {
            entity(session_id)?;
            entity(artifact_id)?;
            let bytes = decode_chunk(chunk_base64, MAX_READ_CHUNK_BYTES)?.len() as u64;
            let end = offset + bytes; // Both counts are bounded to JS safe integers.
            ensure(end <= *total_bytes, "Invalid artifact chunk bounds")?;
            ensure(
                match next_offset {
                    None => end == *total_bytes,
                    Some(next) => bytes > 0 && *next == end && *next < *total_bytes,
                },
                "Invalid artifact chunk continuation",
            )?;
        }
    }
    result_limit(&result)?;
    Ok(result)
}

pub fn decode_delete_result(value: &Value) -> Result<ArtifactDeleteResult> {
    decode_result(value)
}

pub fn result_limit(value: &impl serde::Serialize) -> Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| ProtocolError::invalid(error.to_string()))?;
    ensure(
        bytes.len() <= MAX_RESULT_BYTES,
        "Artifact result exceeds byte limit",
    )
}

fn validate_artifact(artifact: &Artifact) -> Result<()> {
    entity(&artifact.id)?;
    entity(&artifact.session_id)?;
    ensure(
        !artifact.turn_id.is_empty()
            && artifact.turn_id.encode_utf16().count() <= 512
            && !artifact.turn_id.bytes().any(|b| b < 32 || b == 127),
        "Invalid artifact turn key",
    )?;
    text(&artifact.name, 512, false)?;
    if let Some(mime) = &artifact.mime_type {
        text(mime, 512, false)?;
    }
    if let Some(summary) = &artifact.summary {
        text(summary, 8 * 1024, false)?;
    }
    Ok(())
}

fn decode_result<T: DeserializeOwned>(value: &Value) -> Result<T> {
    fn normalize(value: &mut Value) -> Result<()> {
        match value {
            Value::Number(_) => *value = Value::from(codec::count(value, "artifact count")?),
            Value::Array(values) => {
                for value in values {
                    normalize(value)?;
                }
            }
            Value::Object(values) => {
                for value in values.values_mut() {
                    normalize(value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut value = value.clone();
    normalize(&mut value)?;
    serde_json::from_value(value).map_err(|error| ProtocolError::invalid(error.to_string()))
}
