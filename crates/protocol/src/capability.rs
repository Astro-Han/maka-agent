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

pub mod form;
mod manifest;
pub use form::{decode_form_input, decode_form_result};
mod frame;
mod host_frame;
pub use frame::{
    INLINE_RESULT_MAX_BYTES, MAX_PROGRESS_TOTAL, MAX_RESULT_BYTES, MAX_RESULT_CHUNKS,
    RESULT_CHUNK_MAX_BYTES, decode_client_frame, is_client_frame_kind, is_host_frame_kind,
};
pub use host_frame::decode_host_frame;
mod result;
pub mod schema;
pub use result::decode_result;

use crate::{ProtocolError, Result, codec::string};
pub use manifest::{decode_registration_result, decode_replace_input, decode_unregister_input};
use serde_json::Value;

pub const MAX_MANIFEST_BYTES: usize = 56 * 1024;
pub const MAX_OFFERS: usize = 32;
pub const MAX_SERVICES: usize = 32;
pub const MAX_TOOLS: usize = 256;
pub const MAX_TOOLS_PER_OFFER: usize = 64;

fn entity(value: &Value, label: &str) -> Result<String> {
    let id = string(value, label, 128)?;
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err(ProtocolError::invalid(format!("Invalid {label}")));
    }
    Ok(id)
}

pub(super) fn encoded_limit(value: &impl serde::Serialize, limit: usize) -> Result<()> {
    maka_runtime::capability::json::encoded_limit(value, limit).map_err(ProtocolError::invalid)
}

/// serde_json already excludes cycles and non-finite numbers. These extra
/// protocol bounds limit traversal and property names independently of framing.
pub(super) fn validate_json(value: &Value) -> Result<()> {
    fn visit(value: &Value, depth: usize, remaining: &mut usize) -> Result<()> {
        if depth > 32 || *remaining == 0 {
            return Err(ProtocolError::invalid(
                "Client Capability JSON exceeds structural limit",
            ));
        }
        *remaining -= 1;
        match value {
            Value::Array(items) => {
                for item in items {
                    visit(item, depth + 1, remaining)?;
                }
            }
            Value::Object(fields) => {
                for (key, item) in fields {
                    if key.is_empty() || key.encode_utf16().count() > 256 {
                        return Err(ProtocolError::invalid("Invalid Client Capability JSON key"));
                    }
                    visit(item, depth + 1, remaining)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, 0, &mut 8192)
}
