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

use super::validation::{ValidationResult, entity_id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestHeaderUpdate {
    pub name: String,
    #[serde(
        default,
        deserialize_with = "super::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub value: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestHeadersQuery {
    pub connection_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestHeadersReplace {
    pub connection_id: String,
    pub headers: Vec<RequestHeaderUpdate>,
}

impl RequestHeadersReplace {
    pub fn normalize(&mut self) -> ValidationResult {
        entity_id(&self.connection_id)?;
        normalize_updates(&mut self.headers)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequestHeadersQueryResult {
    Found { names: Vec<String> },
    ConnectionNotFound,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequestHeadersReplaceResult {
    Committed { names: Vec<String> },
    Unchanged { names: Vec<String> },
    ConnectionNotFound,
}

pub fn normalize_updates(updates: &mut [RequestHeaderUpdate]) -> ValidationResult {
    if updates.len() > 32 {
        return Err("too many request headers".into());
    }
    let mut seen = BTreeSet::new();
    for update in updates {
        update.name = normalize_name(&update.name)?.into();
        if !seen.insert(update.name.to_ascii_lowercase()) {
            return Err("duplicate request header".into());
        }
        if let Some(value) = &update.value {
            validate_value(value)?;
        }
    }
    Ok(())
}

pub(crate) fn normalize_name(raw: &str) -> ValidationResult<&str> {
    let name = raw.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}');
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&c))
    {
        return Err("invalid header name".into());
    }
    if [
        "authorization",
        "connection",
        "content-length",
        "content-type",
        "host",
        "proxy-authorization",
        "transfer-encoding",
        "x-api-key",
    ]
    .contains(&name.to_ascii_lowercase().as_str())
    {
        return Err("protected request header".into());
    }
    Ok(name)
}

pub(crate) fn validate_value(value: &str) -> ValidationResult {
    if value.is_empty()
        || value.encode_utf16().count() > 8192
        || !value
            .chars()
            .all(|c| c == '\t' || (' '..='~').contains(&c) || ('\u{80}'..='\u{ff}').contains(&c))
    {
        return Err("invalid header value".into());
    }
    Ok(())
}
