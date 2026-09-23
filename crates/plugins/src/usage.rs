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

use crate::{call::Scope, execution::CommandError};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

pub use maka_runtime::accounting::{
    Activity, ActivityKind, ActivityStatus, AuxiliarySource, ModelAttempt, Origin, Outcome,
    Selection, ToolAttempt, ToolResult, ToolStatus,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Filter {
    /// Inclusive finite Unix milliseconds. Fractional timestamps are preserved.
    pub from: f64,
    pub to: f64,
    pub session_id: Option<String>,
    #[serde(default)]
    pub activity: Selection,
}
impl Filter {
    pub fn validate(&self) -> Result<(), CommandError> {
        self.activity
            .validate()
            .map_err(|message| CommandError::Invalid(message.into()))?;
        if !self.from.is_finite() || !self.to.is_finite() || self.from < 0.0 || self.to < self.from
        {
            return Err(CommandError::Invalid("invalid Usage time range".into()));
        }
        if let Some(id) = &self.session_id {
            crate::name(id).map_err(|error| CommandError::Invalid(error.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Read {
    Start {
        filter: Filter,
    },
    /// Opaque continuation, not authority. Invalidated by Host restart.
    Continue {
        cursor: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    /// Re-read this exact page under its original filter and settlement fence.
    pub cursor: String,
    pub next_cursor: Option<String>,
    pub attempts: Vec<Activity>,
    pub total: u64,
}

/// Metadata only: no prompt, response body, credentials or raw event access.
/// Agent calls see their Session; independent calls require read_usage consent.
pub trait Usage: Send + Sync {
    /// At most 100 rows / 48 KiB. Every continuation rechecks current authority.
    fn activity(&self, call: Scope, input: Read) -> BoxFuture<'_, Result<Page, CommandError>>;
}
