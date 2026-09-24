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
pub use maka_plugins::provider::{
    Descriptor, Identity,
    authentication::Method,
    catalog::{Entry, Page, Query},
};
pub use maka_runtime::scope::Scope;
use serde_json::Value;

pub fn validate_query(query: &Query) -> Result<()> {
    Scope::try_from(String::from(query.scope.clone())).map_err(ProtocolError::invalid)?;
    if query.after.is_some() && query.revision.is_none() {
        return Err(ProtocolError::invalid(
            "Provider cursor requires a revision",
        ));
    }
    if let Some(after) = &query.after {
        validate_name(after)?;
    }
    if let Some(revision) = query.revision {
        maka_runtime::configuration::validation::revision(revision, false)
            .map_err(ProtocolError::invalid)?;
    }
    Ok(())
}

pub fn decode_query(value: &Value) -> Result<Query> {
    let query = serde_json::from_value(value.clone())
        .map_err(|_| ProtocolError::invalid("Invalid provider query"))?;
    validate_query(&query)?;
    Ok(query)
}

pub fn decode_page(value: &Value) -> Result<Page> {
    if serde_json::to_vec(value)
        .map_err(|_| ProtocolError::invalid("Invalid provider page"))?
        .len()
        > 512 * 1024 + 4096
    {
        return Err(ProtocolError::invalid("Provider page exceeds its bound"));
    }
    let row = codec::record(value, "provider page")?;
    codec::exact(
        row,
        if value["kind"] == "revision_changed" {
            &["kind", "revision"]
        } else {
            &["kind", "revision", "entries", "next"]
        },
    )?;
    let page: Page = serde_json::from_value(value.clone())
        .map_err(|_| ProtocolError::invalid("Invalid provider page"))?;
    let revision = match &page {
        Page::Page { revision, .. } | Page::RevisionChanged { revision } => *revision,
    };
    maka_runtime::configuration::validation::revision(revision, false)
        .map_err(ProtocolError::invalid)?;
    match &page {
        Page::RevisionChanged { .. } => {}
        Page::Page { entries, next, .. } => {
            if entries.len() > 32 {
                return Err(ProtocolError::invalid("Provider page exceeded its bound"));
            }
            let mut after = None;
            for entry in entries {
                entry.identity.validate().map_err(ProtocolError::invalid)?;
                entry
                    .descriptor
                    .validate()
                    .map_err(|error| ProtocolError::invalid(error.to_string()))?;
                if after.is_some_and(|after| entry.identity.name.as_str() <= after) {
                    return Err(ProtocolError::invalid("Provider page changed cursor order"));
                }
                after = Some(entry.identity.name.as_str());
            }
            if next.as_ref().is_some_and(|next| {
                entries
                    .last()
                    .is_none_or(|entry| &entry.identity.name != next)
            }) {
                return Err(ProtocolError::invalid("Invalid provider continuation"));
            }
        }
    }
    Ok(page)
}

pub fn assert_page(query: &Query, page: &Page) -> Result<()> {
    let valid = match page {
        Page::RevisionChanged { revision } => {
            query.revision.is_some_and(|expected| expected != *revision)
        }
        Page::Page {
            revision, entries, ..
        } => {
            query.revision.is_none_or(|expected| expected == *revision)
                && entries.iter().all(|entry| {
                    query
                        .after
                        .as_ref()
                        .is_none_or(|after| &entry.identity.name > after)
                        && (entry.identity.scope == query.scope
                            || matches!(query.scope, Scope::Session(_))
                                && entry.identity.scope == Scope::Profile)
                })
        }
    };
    if !valid {
        return Err(ProtocolError::invalid(
            "Provider page does not match its query",
        ));
    }
    Ok(())
}

fn validate_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(ProtocolError::invalid("Invalid provider cursor"));
    }
    Ok(())
}
