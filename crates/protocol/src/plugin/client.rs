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

use crate::{ProtocolError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCursor {
    pub revision: String,
    pub after_entry: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ClientQuery {
    Snapshot {
        cursor: Option<ClientCursor>,
    },
    Bundle {
        entry_id: String,
        activation: String,
        client_digest: String,
        offset: usize,
    },
}
impl ClientQuery {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Snapshot {
                cursor: Some(cursor),
            } => {
                digest(&cursor.revision)?;
                identity(&cursor.after_entry)?;
            }
            Self::Snapshot { cursor: None } => {}
            Self::Bundle {
                entry_id,
                activation,
                client_digest,
                offset,
            } => {
                identity(entry_id)?;
                activation_id(activation)?;
                digest(client_digest)?;
                if *offset > maka_plugins::package::MAX_FILE_BYTES {
                    return Err(ProtocolError::invalid(
                        "client bundle offset exceeds its limit",
                    ));
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientDescriptor {
    pub entry_id: String,
    pub extension_id: String,
    pub activation: String,
    pub content_digest: String,
    pub client_digest: String,
    pub sdk_version: u32,
    pub total_bytes: usize,
    pub dependencies: Vec<String>,
    pub config: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ClientResult {
    Snapshot {
        revision: String,
        entries: Vec<ClientDescriptor>,
        next_cursor: Option<ClientCursor>,
    },
    Bundle {
        entry_id: String,
        activation: String,
        client_digest: String,
        offset: usize,
        total_bytes: usize,
        content: String,
        next_offset: Option<usize>,
    },
}
pub(super) fn validate_output(value: &Value) -> Result<()> {
    let result: ClientResult = serde_json::from_value(value.clone())
        .map_err(|error| ProtocolError::invalid(error.to_string()))?;
    match result {
        ClientResult::Snapshot {
            revision,
            entries,
            next_cursor,
        } => {
            digest(&revision)?;
            if entries.len() > 32 {
                return Err(ProtocolError::invalid("client page exceeds 32 entries"));
            }
            let mut previous = None;
            for entry in entries {
                identity(&entry.entry_id)?;
                identity(&entry.extension_id)?;
                activation_id(&entry.activation)?;
                digest(&entry.content_digest)?;
                digest(&entry.client_digest)?;
                if entry.sdk_version == 0
                    || entry.total_bytes > maka_plugins::package::MAX_FILE_BYTES
                    || previous.as_ref().is_some_and(|id| id >= &entry.entry_id)
                {
                    return Err(ProtocolError::invalid(
                        "invalid client descriptor order or limits",
                    ));
                }
                previous = Some(entry.entry_id);
            }
            if let Some(cursor) = next_cursor
                && (cursor.revision != revision || previous.as_deref() != Some(&cursor.after_entry))
            {
                return Err(ProtocolError::invalid("client cursor does not match page"));
            }
        }
        ClientResult::Bundle {
            entry_id,
            activation,
            client_digest,
            offset,
            total_bytes,
            content,
            next_offset,
        } => {
            identity(&entry_id)?;
            activation_id(&activation)?;
            digest(&client_digest)?;
            let end = offset
                .checked_add(content.len())
                .ok_or_else(|| ProtocolError::invalid("client chunk offset overflow"))?;
            if end > total_bytes
                || total_bytes > maka_plugins::package::MAX_FILE_BYTES
                || content.len() > 16 * 1024
                || next_offset != (end < total_bytes).then_some(end)
                || (content.is_empty() && offset != total_bytes)
            {
                return Err(ProtocolError::invalid("invalid client chunk range"));
            }
        }
    }
    Ok(())
}
pub(super) fn digest(value: &str) -> Result<()> {
    let valid = value.strip_prefix("sha256-").is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::invalid("invalid client content digest"))
    }
}
pub(super) fn identity(value: &str) -> Result<()> {
    maka_plugins::identifier(value).map_err(|error| ProtocolError::invalid(error.to_string()))
}
pub(super) fn activation_id(value: &str) -> Result<()> {
    if value.len() == 36
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        Ok(())
    } else {
        Err(ProtocolError::invalid("invalid client activation identity"))
    }
}
