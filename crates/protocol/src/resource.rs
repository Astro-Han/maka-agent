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

use crate::{OperationErrorCode as Code, ProtocolError, Result};
use maka_presentation::shell::{Ownership, ResourceUpdate};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
mod mutation;
pub use mutation::*;
mod controller;
pub use controller::*;

pub const MAX_RESULT_BYTES: usize = 52 * 1024;
pub const MAX_PAGE_ITEMS: usize = 64;
pub const QUERY_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::NotFound,
    Code::InvalidRequest,
    Code::InternalFailure,
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ResourceQueryInput {
    ListStart {
        session_id: String,
    },
    ListContinue {
        session_id: String,
        revision: String,
        cursor: String,
    },
    Get {
        session_id: String,
        #[serde(rename = "ref")]
        resource_ref: String,
    },
}
impl ResourceQueryInput {
    pub fn session_id(&self) -> &str {
        match self {
            Self::ListStart { session_id }
            | Self::ListContinue { session_id, .. }
            | Self::Get { session_id, .. } => session_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ResourceQueryResult {
    RevisionChanged {
        expected: String,
        actual: String,
    },
    Resource {
        session_id: String,
        revision: String,
        #[serde(deserialize_with = "nullable")]
        resource: Option<Box<ResourceUpdate>>,
    },
    Page {
        session_id: String,
        revision: String,
        resources: Vec<ResourceUpdate>,
        #[serde(deserialize_with = "nullable")]
        next_cursor: Option<String>,
    },
}

pub fn decode_query_input(value: &Value) -> Result<ResourceQueryInput> {
    let input: ResourceQueryInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    entity(input.session_id())?;
    match &input {
        ResourceQueryInput::ListStart { .. } => {}
        ResourceQueryInput::ListContinue {
            revision, cursor, ..
        } => {
            digest(revision)?;
            text(cursor, 32)?;
        }
        ResourceQueryInput::Get { resource_ref, .. } => text(resource_ref, 256)?,
    }
    Ok(input)
}

pub fn decode_query_result(value: &Value) -> Result<ResourceQueryResult> {
    let output: ResourceQueryResult = serde_json::from_value(value.clone()).map_err(invalid)?;
    match &output {
        ResourceQueryResult::RevisionChanged { expected, actual } => {
            digest(expected)?;
            digest(actual)?;
        }
        ResourceQueryResult::Resource {
            session_id,
            revision,
            resource,
        } => {
            entity(session_id)?;
            digest(revision)?;
            if let Some(resource) = resource {
                validate_update(resource)?;
            }
        }
        ResourceQueryResult::Page {
            session_id,
            revision,
            resources,
            next_cursor,
        } => {
            entity(session_id)?;
            digest(revision)?;
            if resources.len() > MAX_PAGE_ITEMS {
                return Err(invalid("resource page exceeds item limit"));
            }
            for resource in resources {
                validate_update(resource)?;
            }
            if let Some(cursor) = next_cursor {
                text(cursor, 32)?;
            }
        }
    }
    if serde_json::to_vec(&output).map_err(invalid)?.len() > MAX_RESULT_BYTES {
        return Err(invalid("resource result exceeds byte limit"));
    }
    Ok(output)
}
fn validate_update(update: &ResourceUpdate) -> Result<()> {
    entity(&update.session_id)?;
    entity(&update.source_turn_id)?;
    text(&update.source_tool_call_id, 512)?;
    match &update.ownership {
        Ownership::Local => {}
        Ownership::SourceOwned {
            source_session_id,
            owner_session_id,
        } => {
            entity(source_session_id)?;
            entity(owner_session_id)?;
        }
        Ownership::SourceUnavailable { source_session_id } => entity(source_session_id)?,
    }
    update.result.validate().map_err(invalid)
}
fn entity(value: &str) -> Result<()> {
    maka_runtime::interaction::entity_id(value).map_err(invalid)
}
fn digest(value: &str) -> Result<()> {
    if value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        Ok(())
    } else {
        Err(invalid("invalid resource revision"))
    }
}
fn text(value: &str, max: usize) -> Result<()> {
    if value.is_empty() || value.len() > max {
        Err(invalid("invalid resource text"))
    } else {
        Ok(())
    }
}
fn invalid(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::invalid(error.to_string())
}
fn nullable<'de, D, T>(de: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(de)
}
