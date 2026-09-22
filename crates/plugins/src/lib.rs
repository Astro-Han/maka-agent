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

//! Plugin composition and lifecycle contracts, independent of business policy.

pub mod authorization;
pub mod background;
pub mod call;
pub mod client;
pub mod client_capability;
pub mod composition;
pub mod contributions;
pub mod credentials;
pub mod execution;
pub mod executor;
pub mod fiber;
pub mod filesystem;
pub mod host;
pub mod http;
pub mod kernel;
pub mod llm;
pub mod model;
pub mod package;
pub mod permissions;
pub mod preferences;
pub mod process;
pub mod prompt;
mod registration;
pub mod remote;
pub mod revision;
pub use registration::Registration;
pub mod input;
pub mod services;
pub mod session;
pub mod storage;
pub mod terminal;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("plugin instance is retired or not effective")]
    Retired,
    #[error("invalid plugin lifecycle transition: {0}")]
    Lifecycle(&'static str),
    #[error("plugin cleanup has not completed before the deadline")]
    CleanupPending,
    #[error("plugin cleanup failed: {0:?}")]
    Cleanup(Vec<String>),
    #[error("plugin service is already registered: {0}")]
    DuplicateService(String),
    #[error("plugin service has a different Rust type: {0}")]
    ServiceType(String),
    #[error("plugin contribution conflicts with an existing or reserved name: {0}")]
    ContributionConflict(String),
    #[error("invalid plugin composition: {0}")]
    Invalid(String),
    #[error("composition entry not found: {0}")]
    MissingEntry(String),
    #[error("duplicate composition entry: {0}")]
    DuplicateEntry(String),
    #[error("composition entries cannot move between roots")]
    CrossRoot,
    #[error("composition entry cannot contain itself")]
    DependencyCycle,
}

pub(crate) fn name(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(Error::Invalid(format!("invalid identifier: {value:?}")));
    }
    Ok(())
}

/// Validate a stable package or composition Entry identifier.
pub fn identifier(value: &str) -> Result<(), Error> {
    if value.len() > 128
        || !value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        || value.split(['.', '_', ':', '-']).any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
    {
        return Err(Error::Invalid(format!(
            "invalid plugin identifier: {value:?}"
        )));
    }
    Ok(())
}
