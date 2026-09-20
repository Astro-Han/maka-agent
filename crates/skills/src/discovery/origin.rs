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
    DiscoveryFailure,
    source::{ReadError, read_view},
};
use maka_runtime::artifact::content_digest;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

const MAX_LOCK_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct Origin {
    pub lock_sha256: Option<String>,
    pub status: OriginStatus,
}

#[derive(Debug, Clone, Serialize)]
pub enum OriginStatus {
    Missing,
    Invalid(OriginFailure),
    Bundled {
        content_sha256: String,
    },
    Managed {
        source_id: String,
        content_sha256: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OriginFailure {
    InvalidJson,
    UnsupportedSchema,
    IdMismatch,
    InvalidHash,
    UnsafePath,
    ReadFailed,
    TooLarge,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Lock {
    schema_version: u64,
    id: String,
    source_type: SourceType,
    source_name: String,
    source_version: String,
    content_sha256: String,
    source_id: Option<String>,
    source_content_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum SourceType {
    Bundled,
    Managed,
    #[serde(other)]
    Other,
}

pub(super) fn read(
    reader: &maka_plugins::filesystem::Reader<'_>,
    directory: &std::path::Path,
    id: &str,
    cancellation: &CancellationToken,
) -> Result<(Origin, usize), ReadError> {
    let invalid = |reason| {
        (
            Origin {
                lock_sha256: None,
                status: OriginStatus::Invalid(reason),
            },
            0,
        )
    };
    let bytes = match read_view(
        reader,
        &directory.join("skill.lock.json"),
        MAX_LOCK_BYTES as usize,
        cancellation,
    ) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return Ok((
                Origin {
                    lock_sha256: None,
                    status: OriginStatus::Missing,
                },
                0,
            ));
        }
        Err(ReadError::Cancelled) => return Err(ReadError::Cancelled),
        Err(ReadError::Failure(reason)) => {
            return Ok(invalid(match reason {
                DiscoveryFailure::BlockedPath => OriginFailure::UnsafePath,
                DiscoveryFailure::ReadFailed => OriginFailure::ReadFailed,
                DiscoveryFailure::SourceTooLarge => OriginFailure::TooLarge,
            }));
        }
    };
    let status = parse(&bytes, id).unwrap_or_else(OriginStatus::Invalid);
    let origin = Origin {
        lock_sha256: Some(content_digest(&bytes)),
        status,
    };
    Ok((origin, bytes.len()))
}

fn parse(bytes: &[u8], id: &str) -> Result<OriginStatus, OriginFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| OriginFailure::InvalidJson)?;
    let lock: Lock = serde_json::from_str(text).map_err(|_| OriginFailure::InvalidJson)?;
    if lock.schema_version != 1 {
        return Err(OriginFailure::UnsupportedSchema);
    }
    if lock.id != id {
        return Err(OriginFailure::IdMismatch);
    }
    if !is_hash(&lock.content_sha256) {
        return Err(OriginFailure::InvalidHash);
    }
    let content_sha256 = lock.content_sha256.to_ascii_lowercase();
    match lock.source_type {
        SourceType::Managed => {
            let source_id = lock
                .source_id
                .filter(|id| super::safe_source_id(id))
                .ok_or(OriginFailure::UnsupportedSchema)?;
            if lock.source_name != "local-library"
                || lock.source_version != "1"
                || !lock
                    .source_content_sha256
                    .as_deref()
                    .is_some_and(|hash| is_hash(hash) && hash.eq_ignore_ascii_case(&content_sha256))
            {
                return Err(OriginFailure::UnsupportedSchema);
            }
            Ok(OriginStatus::Managed {
                source_id,
                content_sha256,
            })
        }
        SourceType::Bundled
            if lock.source_name == "maka-bundled"
                && lock.source_version == "1"
                && super::sources::trusted_bundled_hash(id, &content_sha256) =>
        {
            Ok(OriginStatus::Bundled { content_sha256 })
        }
        _ => Err(OriginFailure::UnsupportedSchema),
    }
}

fn is_hash(value: &str) -> bool {
    value.len() == 71
        && value
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("sha256:"))
        && value.as_bytes()[7..].iter().all(u8::is_ascii_hexdigit)
}
