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

use super::validate_json;
use crate::{
    ProtocolError, Result,
    codec::{exact, record, shaped, string},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use maka_runtime::capability::{CallResult, ContentBlock};
use serde_json::Value;

const MAX_RESULT_CHARS: usize = 24 * 1024 * 1024;

/// Decode content after the caller applies its inline or assembled byte budget.
pub fn decode_result(value: &Value) -> Result<CallResult> {
    let fields = record(value, "Client Capability result")?;
    shaped(fields, &["content"], &["structuredContent"])?;
    let content = fields["content"]
        .as_array()
        .filter(|items| items.len() <= 256)
        .ok_or_else(|| ProtocolError::invalid("Invalid Client Capability result content"))?
        .iter()
        .map(decode_content_block)
        .collect::<Result<Vec<_>>>()?;
    let structured_content = fields
        .get("structuredContent")
        .map(|value| {
            validate_json(value)?;
            Ok(value.clone())
        })
        .transpose()?;
    Ok(CallResult {
        content,
        structured_content,
    })
}

fn decode_content_block(value: &Value) -> Result<ContentBlock> {
    let fields = record(value, "Client Capability content block")?;
    let optional_string = |key: &str, max| {
        fields
            .get(key)
            .map(|value| string(value, key, max))
            .transpose()
    };
    match fields.get("type").and_then(Value::as_str) {
        Some("text") => {
            exact(fields, &["type", "text"])?;
            Ok(ContentBlock::Text {
                text: bounded_string(&fields["text"], "text")?.into(),
            })
        }
        Some(kind @ ("image" | "audio")) => {
            exact(fields, &["type", "data", "mimeType"])?;
            let data = canonical_base64(&fields["data"], "data")?;
            let mime_type = string(&fields["mimeType"], "mimeType", 256)?;
            if kind == "image" {
                let subtype = mime_type.strip_prefix("image/").unwrap_or_default();
                if !subtype
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                    || !subtype
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
                {
                    return Err(ProtocolError::invalid(
                        "Invalid Client Capability image MIME type",
                    ));
                }
                Ok(ContentBlock::Image { data, mime_type })
            } else {
                Ok(ContentBlock::Audio { data, mime_type })
            }
        }
        Some("resource") => {
            shaped(fields, &["type", "uri"], &["mimeType", "text", "blob"])?;
            Ok(ContentBlock::Resource {
                uri: string(&fields["uri"], "uri", 4096)?,
                mime_type: optional_string("mimeType", 256)?,
                text: fields
                    .get("text")
                    .map(|value| bounded_string(value, "text").map(str::to_owned))
                    .transpose()?,
                blob: fields
                    .get("blob")
                    .map(|value| canonical_base64(value, "blob"))
                    .transpose()?,
            })
        }
        Some("resource_link") => {
            shaped(
                fields,
                &["type", "uri"],
                &["name", "description", "mimeType"],
            )?;
            Ok(ContentBlock::ResourceLink {
                uri: string(&fields["uri"], "uri", 4096)?,
                name: optional_string("name", 512)?,
                description: optional_string("description", 4096)?,
                mime_type: optional_string("mimeType", 256)?,
            })
        }
        Some("unknown") => {
            exact(fields, &["type", "value"])?;
            validate_json(&fields["value"])?;
            Ok(ContentBlock::Unknown {
                value: fields["value"].clone(),
            })
        }
        _ => Err(ProtocolError::invalid(
            "Invalid Client Capability content block type",
        )),
    }
}

fn bounded_string<'a>(value: &'a Value, label: &str) -> Result<&'a str> {
    value
        .as_str()
        .filter(|s| s.encode_utf16().take(MAX_RESULT_CHARS + 1).count() <= MAX_RESULT_CHARS)
        .ok_or_else(|| ProtocolError::invalid(format!("Invalid {label}")))
}

fn canonical_base64(value: &Value, label: &str) -> Result<String> {
    let data = bounded_string(value, label)?;
    // STANDARD requires padding and rejects nonzero trailing pad bits.
    if data.is_empty() || STANDARD.decode(data).is_err() {
        return Err(ProtocolError::invalid(format!("Invalid {label}")));
    }
    Ok(data.into())
}
