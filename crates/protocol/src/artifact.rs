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

mod input;
mod output;
mod preview;
mod result_codec;
use crate::{ProtocolError, Result, codec};
use base64::{
    Engine as _, alphabet,
    engine::{GeneralPurpose, GeneralPurposeConfig},
};
pub use input::*;
use maka_runtime::attachment::MAX_ATTACHMENT_BYTES;
pub use output::*;
pub use result_codec::*;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub const MAX_INGEST_CHUNK_BYTES: usize = 48 * 1024;
pub const MAX_READ_CHUNK_BYTES: usize = 32 * 1024;
pub const MAX_PREVIEW_BYTES: usize = 32 * 1024;
pub const MAX_RESULT_BYTES: usize = 48 * 1024;
pub const MAX_PAGE_ITEMS: usize = 128;

// The source protocol requires padding and the standard alphabet, but its
// regex + Buffer decoder accepts nonzero unused bits. Preserve that contract.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
);

pub fn decode_ingest_input(value: &Value) -> Result<ArtifactIngestInput> {
    let input: ArtifactIngestInput = decode(value)?;
    entity(input.session_id())?;
    entity(input.upload_id())?;
    match &input {
        ArtifactIngestInput::Begin {
            name,
            mime_type,
            total_bytes,
            content_sha256,
            ..
        } => {
            ingest_text(name, 512)?;
            ingest_text(mime_type, 256)?;
            ensure(
                *total_bytes <= MAX_ATTACHMENT_BYTES,
                "Attachment exceeds byte limit",
            )?;
            digest(content_sha256)?;
        }
        ArtifactIngestInput::Chunk { chunk_base64, .. } => {
            let bytes = decode_chunk(chunk_base64, MAX_INGEST_CHUNK_BYTES)?;
            ensure(!bytes.is_empty(), "Artifact ingest chunk cannot be empty")?;
        }
        ArtifactIngestInput::Commit { .. } | ArtifactIngestInput::Abort { .. } => {}
    }
    Ok(input)
}

pub fn decode_query_input(value: &Value) -> Result<ArtifactQueryInput> {
    let input: ArtifactQueryInput = decode(value)?;
    entity(input.session_id())?;
    match &input {
        ArtifactQueryInput::ListStart { .. } => {}
        ArtifactQueryInput::ListContinue {
            revision, cursor, ..
        } => {
            digest(revision)?;
            text(cursor, 32, false)?;
        }
        ArtifactQueryInput::Get { artifact_id, .. }
        | ArtifactQueryInput::ReadText { artifact_id, .. }
        | ArtifactQueryInput::ReadBinary { artifact_id, .. }
        | ArtifactQueryInput::ReadChunk { artifact_id, .. } => entity(artifact_id)?,
    }
    Ok(input)
}

pub fn decode_delete_input(value: &Value) -> Result<ArtifactDeleteInput> {
    let input: ArtifactDeleteInput = decode(value)?;
    entity(&input.session_id)?;
    entity(&input.artifact_id)?;
    Ok(input)
}

pub fn decode_chunk(value: &str, limit: usize) -> Result<Vec<u8>> {
    ensure(
        value.len() <= limit.div_ceil(3) * 4 + 4,
        "Artifact chunk exceeds byte limit",
    )?;
    let bytes = BASE64
        .decode(value)
        .map_err(|_| ProtocolError::invalid("Invalid artifact base64"))?;
    ensure(bytes.len() <= limit, "Artifact chunk exceeds byte limit")?;
    Ok(bytes)
}

fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    // These command inputs are flat records with integer-only numeric fields.
    let mut value = codec::record(value, "artifact input")?.clone();
    for value in value.values_mut() {
        if value.is_number() {
            *value = Value::from(codec::count(value, "artifact count")?);
        }
    }
    serde_json::from_value(Value::Object(value))
        .map_err(|error| ProtocolError::invalid(error.to_string()))
}
fn entity(value: &str) -> Result<()> {
    maka_runtime::interaction::entity_id(value).map_err(ProtocolError::invalid)
}
fn digest(value: &str) -> Result<()> {
    ensure(
        value.strip_prefix("sha256:").is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }),
        "Invalid artifact digest",
    )
}
fn text(value: &str, limit: usize, empty: bool) -> Result<()> {
    ensure(
        (empty || !value.is_empty()) && value.len() <= limit,
        "Invalid artifact text",
    )
}
fn ingest_text(value: &str, limit: usize) -> Result<()> {
    text(value, limit, false)?;
    ensure(
        !value.bytes().any(|b| b < 32 || b == 127),
        "Control character in artifact metadata",
    )
}
fn ensure(valid: bool, message: &str) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::invalid(message))
    }
}
