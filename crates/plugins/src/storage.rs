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

use crate::{Error, composition::Scope, identifier};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod directory;
pub use directory::{Directories, Directory};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Namespace {
    package: String,
    scope: Scope,
}

impl Namespace {
    pub fn new(package: impl Into<String>, scope: Scope) -> Result<Self, Error> {
        let package = package.into();
        identifier(&package)?;
        if let Scope::Session(id) = &scope {
            crate::name(id)?;
        }
        Ok(Self { package, scope })
    }
    pub fn package(&self) -> &str {
        &self.package
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
}

/// Deletions retain a revision, preventing stale writers from recreating a key
/// as if it had never existed. Revision is not a business schema version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub revision: u64,
    pub data: Data,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Data {
    Present(Value),
    Deleted,
}

impl Data {
    pub fn value(&self) -> Option<&Value> {
        match self {
            Self::Present(value) => Some(value),
            Self::Deleted => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mutation {
    pub key: String,
    pub expected_revision: Option<u64>,
    pub data: Data,
}

impl Mutation {
    pub fn validate(&self) -> Result<(), Error> {
        validate_key(&self.key)?;
        if self
            .expected_revision
            .is_some_and(|revision| revision == 0 || revision >= (1 << 53) - 1)
        {
            return Err(Error::Invalid("invalid plugin data revision".into()));
        }
        if serde_json::to_vec(&self.data)
            .map_err(|error| Error::Invalid(error.to_string()))?
            .len()
            > 1024 * 1024
        {
            return Err(Error::Invalid("plugin data value exceeds 1 MiB".into()));
        }
        Ok(())
    }
}

pub fn validate_key(key: &str) -> Result<(), Error> {
    if key.is_empty() || key.len() > 1024 || key.chars().any(char::is_control) {
        return Err(Error::Invalid("invalid plugin storage key".into()));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("plugin storage capability is retired")]
    Retired,
    #[error("plugin data revision conflict: expected {expected}, found {actual}")]
    Conflict { expected: String, actual: String },
    #[error("plugin data commit outcome is unknown: {0}")]
    OutcomeUnknown(String),
    #[error("plugin storage is unavailable: {0}")]
    Unavailable(String),
}

/// Namespace is bound by Host, not supplied by each plugin operation.
pub trait Store: Send + Sync {
    fn read(
        &self,
        key: String,
    ) -> futures_util::future::BoxFuture<'_, Result<Option<Record>, StoreError>>;
    fn batch(
        &self,
        mutations: Vec<Mutation>,
    ) -> futures_util::future::BoxFuture<'_, Result<Vec<Record>, StoreError>>;
}
