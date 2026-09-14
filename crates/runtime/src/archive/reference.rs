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
    ArchiveIdentity, MAX_ARCHIVE_REF_CHARS, hash, identity_string, valid_projection_digest,
};
const PREFIX: &str = "maka://archive-ledger/v1/";
const SHORT_PREFIX: &str = "archive:";
const EVENT_PREFIX: &str = "maka://runtime/tool-results/";

/// A Session-scoped locator or an exact accepted archive proof; neither grants access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolResultAddress {
    Event(String),
    Evidence(ArchiveIdentity),
}

impl ToolResultAddress {
    pub fn parse(path: &str) -> Result<Self, &'static str> {
        if path.len() > MAX_ARCHIVE_REF_CHARS {
            return Err("tool result address exceeds limit");
        }
        if path.starts_with(PREFIX) {
            return ArchiveIdentity::parse_resource_ref(path).map(Self::Evidence);
        }
        if path.starts_with(SHORT_PREFIX) {
            return ArchiveIdentity::parse_short_ref(path).map(Self::Event);
        }
        let encoded = path
            .strip_prefix(EVENT_PREFIX)
            .ok_or("invalid tool result address")?;
        let id = String::from_utf8(decode(encoded)?).map_err(|_| "invalid tool result event ID")?;
        if !identity_string(&id) || encode(&id) != encoded {
            return Err("invalid or noncanonical tool result event ID");
        }
        Ok(Self::Event(id))
    }

    pub fn event_path(id: &str) -> Result<String, &'static str> {
        if !identity_string(id) {
            return Err("invalid tool result event ID");
        }
        Ok(format!("{EVENT_PREFIX}{}", encode(id)))
    }
}

impl ArchiveIdentity {
    /// A locator only. Session access and full evidence are resolved by the store.
    pub fn short_ref(&self) -> Result<String, &'static str> {
        self.validate()?;
        Ok(format!("{SHORT_PREFIX}{}", encode(&self.runtime_event_id)))
    }

    pub fn parse_short_ref(resource: &str) -> Result<String, &'static str> {
        if resource.len() > MAX_ARCHIVE_REF_CHARS {
            return Err("archive reference exceeds limit");
        }
        let encoded = resource
            .strip_prefix(SHORT_PREFIX)
            .ok_or("unsupported archive reference")?;
        let id = String::from_utf8(decode(encoded)?).map_err(|_| "invalid archive event ID")?;
        if !identity_string(&id) || encode(&id) != encoded {
            return Err("invalid or noncanonical archive event ID");
        }
        Ok(id)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if ![&self.runtime_event_id, &self.tool_call_id, &self.tool_name]
            .into_iter()
            .all(|value| identity_string(value))
            || !valid_projection_digest(&self.source_projection_digest)
            || !hash(&self.body_sha256)
            || !(1..=9_007_199_254_740_991).contains(&self.original_bytes)
        {
            return Err("invalid ledger archive identity");
        }
        Ok(())
    }

    pub fn resource_ref(&self) -> Result<String, &'static str> {
        self.validate()?;
        let tuple = (
            &self.runtime_event_id,
            &self.tool_call_id,
            &self.tool_name,
            &self.source_projection_digest,
            Option::<&str>::None,
            &self.body_sha256,
            self.original_bytes,
        );
        let json = serde_json::to_string(&tuple).map_err(|_| "invalid archive identity")?;
        let result = format!("{PREFIX}{}", encode(&json));
        if result.len() > MAX_ARCHIVE_REF_CHARS {
            return Err("archive reference exceeds limit");
        }
        Ok(result)
    }

    pub fn parse_resource_ref(resource: &str) -> Result<Self, &'static str> {
        if resource.len() > MAX_ARCHIVE_REF_CHARS {
            return Err("archive reference exceeds limit");
        }
        let encoded = resource
            .strip_prefix(PREFIX)
            .ok_or("unsupported archive reference")?;
        let bytes = decode(encoded)?;
        let (
            runtime_event_id,
            tool_call_id,
            tool_name,
            source_projection_digest,
            previous,
            body_sha256,
            original_bytes,
        ): (String, String, String, String, Option<String>, String, u64) =
            serde_json::from_slice(&bytes).map_err(|_| "invalid archive identity")?;
        // Rust creates exactly one base replacement; legacy and chained identities
        // are not evidence of an accepted replacement in this runtime.
        if previous.is_some() {
            return Err("archive predecessor is unsupported");
        }
        let identity = Self {
            runtime_event_id,
            tool_call_id,
            tool_name,
            source_projection_digest,
            body_sha256,
            original_bytes,
        };
        if identity.resource_ref()? != resource {
            return Err("noncanonical archive reference");
        }
        Ok(identity)
    }
}

fn encode(value: &str) -> String {
    let mut result = String::new();
    // ECMAScript encodeURIComponent, deliberately not form-urlencoded.
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            result.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(&mut result, "%{byte:02X}").expect("String write");
        }
    }
    result
}
fn decode(encoded: &str) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut input = encoded.bytes();
    while let Some(byte) = input.next() {
        bytes.push(if byte == b'%' {
            let high = input
                .next()
                .and_then(hex)
                .ok_or("invalid percent encoding")?;
            let low = input
                .next()
                .and_then(hex)
                .ok_or("invalid percent encoding")?;
            high * 16 + low
        } else {
            byte
        });
    }
    Ok(bytes)
}
fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|value| value as u8)
}
