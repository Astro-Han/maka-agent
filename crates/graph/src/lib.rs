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

//! Agent Graph business policy. Host execution and plugin lifecycle are separate authorities.
pub mod control;
pub mod coordinator;
pub mod decision;
pub mod owner;
pub mod projection;
pub mod schedule;
pub mod store;
pub mod swarm;
pub mod view;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Agent Graph input: {0}")]
    Invalid(String),
    #[error("Agent Graph is closed")]
    Closed,
    #[error("Agent Graph revision changed")]
    Conflict,
    #[error("Agent Graph item not found: {0}")]
    NotFound(String),
    #[error("Agent Graph persistence failed: {0}")]
    Persistence(String),
    #[error(transparent)]
    Host(#[from] maka_plugins::execution::CommandError),
}

pub fn identity(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(Error::Invalid("invalid identity".into()));
    }
    Ok(())
}

macro_rules! id {
    ($ty:ident, $prefix:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $ty(String);

        impl $ty {
            pub fn new() -> Self {
                Self(format!("{}{}", $prefix, uuid::Uuid::new_v4().simple()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl Default for $ty {
            fn default() -> Self {
                Self::new()
            }
        }
        impl TryFrom<String> for $ty {
            type Error = Error;
            fn try_from(value: String) -> Result<Self, Error> {
                identity(&value)?;
                if !value.starts_with($prefix) {
                    return Err(Error::Invalid(concat!("invalid ", stringify!($ty)).into()));
                }
                Ok(Self(value))
            }
        }
        impl From<$ty> for String {
            fn from(id: $ty) -> Self {
                id.0
            }
        }
        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id!(GraphId, "agent_graph_");
id!(WorkId, "graph_work_");
id!(OperatorId, "graph_operator_");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Graph,
    Swarm,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Epoch {
    pub root_session_id: String,
    pub epoch: u64,
    pub graph_id: GraphId,
    pub created_at: u64,
    pub mode: Mode,
}

impl Epoch {
    pub fn validate(&self) -> Result<(), Error> {
        identity(&self.root_session_id)?;
        if self.epoch == 0 || self.epoch > (1 << 53) - 1 || self.created_at > (1 << 53) - 1 {
            return Err(Error::Invalid("invalid graph epoch or time".into()));
        }
        Ok(())
    }
}
