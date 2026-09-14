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

use super::codec::ensure;
use super::*;
use crate::{
    ProtocolError, Result,
    codec::{count, exact, record},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

pub(super) fn decode_fragment(
    v: &Value,
    source: SessionTranscriptPageSource,
    watermark: Option<u64>,
) -> Result<(SessionTranscriptFragment, u64)> {
    let fields = match source {
        SessionTranscriptPageSource::Durable => &[
            "kind",
            "sequence",
            "byteOffset",
            "totalBytes",
            "payloadDigest",
            "data",
        ][..],
        SessionTranscriptPageSource::Overlay => {
            &["kind", "messageIndex", "byteOffset", "totalBytes", "data"][..]
        }
    };
    exact(record(v, "transcript fragment")?, fields)?;
    ensure(
        super::codec::source(&v["kind"])? == source,
        "Fragment source changed",
    )?;
    let byte_offset = count(&v["byteOffset"], "byteOffset")?;
    let total_bytes = count(&v["totalBytes"], "totalBytes")?;
    let data = v["data"]
        .as_str()
        .filter(|s| {
            !s.is_empty() && s.len() <= (SESSION_TRANSCRIPT_PAGE_MAX_BYTES as usize).div_ceil(3) * 4
        })
        .ok_or_else(|| ProtocolError::invalid("Invalid fragment data"))?;
    let decoded = STANDARD
        .decode(data)
        .map_err(|_| ProtocolError::invalid("Invalid fragment base64"))?;
    let size = decoded.len() as u64;
    ensure(
        size > 0 && size <= SESSION_TRANSCRIPT_PAGE_MAX_BYTES && STANDARD.encode(&decoded) == data,
        "Invalid fragment base64",
    )?;
    ensure(
        total_bytes > 0
            && byte_offset < total_bytes
            && byte_offset
                .checked_add(size)
                .is_some_and(|end| end <= total_bytes),
        "Invalid fragment bounds",
    )?;
    let data = data.to_owned();
    let fragment = match source {
        SessionTranscriptPageSource::Durable => {
            let sequence = count(&v["sequence"], "sequence")?;
            ensure(
                watermark.is_some_and(|w| sequence <= w),
                "Fragment exceeds watermark",
            )?;
            let payload_digest = if v["payloadDigest"].is_null() {
                None
            } else {
                let digest = v["payloadDigest"]
                    .as_str()
                    .ok_or_else(|| ProtocolError::invalid("Invalid payload digest"))?;
                ensure(
                    digest.strip_prefix("sha256:").is_some_and(|s| {
                        s.len() == 64
                            && s.bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    }),
                    "Invalid payload digest",
                )?;
                Some(digest.to_owned())
            };
            SessionTranscriptFragment::Durable {
                sequence,
                byte_offset,
                total_bytes,
                payload_digest,
                data,
            }
        }
        SessionTranscriptPageSource::Overlay => SessionTranscriptFragment::Overlay {
            message_index: count(&v["messageIndex"], "messageIndex")?,
            byte_offset,
            total_bytes,
            data,
        },
    };
    Ok((fragment, size))
}
