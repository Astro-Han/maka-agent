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

use crate::{ProtocolError, Result, codec};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::HashSet;

pub(super) fn decode<T: DeserializeOwned + Serialize>(value: &Value) -> Result<T> {
    let mut normalized = value.clone();
    validate(&mut normalized, "")?;
    let result: T =
        serde_json::from_value(normalized).map_err(|e| ProtocolError::invalid(e.to_string()))?;
    Ok(result)
}

fn invalid() -> ProtocolError {
    ProtocolError::invalid("Invalid Session contract value")
}

fn validate(value: &mut Value, field: &str) -> Result<()> {
    if value.is_null() {
        return if ["llmConnectionId", "nextCursor", "session"].contains(&field) {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    match value {
        Value::Object(map) => {
            if matches!(
                map.get("kind").and_then(Value::as_str),
                Some("list_start" | "default")
            ) {
                codec::exact(map, &["kind"])?;
            }
            let projection = map.contains_key("workspace") && map.contains_key("id");
            let page = map.get("kind").and_then(Value::as_str) == Some("page");
            if projection && !map.contains_key("llmConnectionId")
                || page && !map.contains_key("nextCursor")
                || map.get("kind").and_then(Value::as_str) == Some("session")
                    && !map.contains_key("session")
            {
                return Err(invalid());
            }
            for (key, v) in map.iter_mut() {
                validate(v, key)?;
            }
            if let (Some(target), Some(cwd)) = (map.get("target"), map.get("hostCwd"))
                && target["kind"] == "host_path"
                && target["path"] != *cwd
            {
                return Err(invalid());
            }
            if projection || page {
                let bytes = serde_json::to_vec(&map).map_err(|_| invalid())?;
                if bytes.len() > 48 * 1024 {
                    return Err(invalid());
                }
            }
        }
        Value::Array(items) => {
            let maximum = match field {
                "sessions" => 32,
                "labels" => 32,
                "runningTurnIds" => 64,
                _ => return Err(invalid()),
            };
            if items.len() > maximum {
                return Err(invalid());
            }
            let mut seen = HashSet::new();
            for item in items {
                validate(item, field)?;
                if field != "sessions" {
                    let text = item.as_str().ok_or_else(invalid)?;
                    if !seen.insert(text.to_owned()) {
                        return Err(invalid());
                    }
                }
            }
        }
        Value::Number(_) => {
            let n = codec::count(value, field)?;
            if [
                "revision",
                "expectedRevision",
                "actualRevision",
                "revisionIndex",
            ]
            .contains(&field)
                && n == 0
                || field == "schemaVersion" && n != 1
            {
                return Err(invalid());
            }
            *value = Value::from(n);
        }
        Value::String(text) => match field {
            "id"
            | "sessionId"
            | "projectId"
            | "connectionId"
            | "llmConnectionId"
            | "parentSessionId"
            | "branchOfTurnId"
            | "revisionRootSessionId"
            | "revisionParentSessionId"
            | "revisionOfTurnId"
            | "lastReadMessageId"
            | "readThroughMessageId"
            | "runningTurnIds" => entity(text)?,
            "revision" | "expectedRevision" | "actualRevision" => {
                let hex = text.strip_prefix("sha256:").ok_or_else(invalid)?;
                if hex.len() != 64
                    || !hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(invalid());
                }
            }
            "path" | "hostCwd" => {
                utf8(text, 4096)?;
                if !codec::absolute_host_path(text) {
                    return Err(invalid());
                }
            }
            "name" => bounded(text, 320)?,
            "labels" => bounded(text, 128)?,
            "lastMessagePreview" => bounded(text, 4096)?,
            "agentId" | "agentName" | "profile" => bounded(text, 512)?,
            "connectionSlug" | "llmConnectionSlug" => utf8(text, 256)?,
            "model" => utf8(text, 512)?,
            "cursor" | "nextCursor" => utf8(text, 512)?,
            "status" if text == "review" || text == "done" => *text = "active".into(),
            _ => {}
        },
        Value::Bool(_) => {}
        Value::Null => unreachable!(),
    }
    Ok(())
}

fn entity(text: &str) -> Result<()> {
    if text.is_empty()
        || text.len() > 128
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid());
    }
    Ok(())
}
fn utf8(text: &str, max: usize) -> Result<()> {
    if text.is_empty() || text.len() > max {
        return Err(invalid());
    }
    Ok(())
}
fn bounded(text: &str, max: usize) -> Result<()> {
    utf8(text, max)?;
    // ECMAScript trim includes BOM, but excludes Rust's NEL whitespace.
    let whitespace = |c: char| matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}');
    if text.trim_matches(whitespace) != text
        || text.chars().any(|c| c <= '\u{001f}' || c == '\u{007f}')
    {
        return Err(invalid());
    }
    Ok(())
}
