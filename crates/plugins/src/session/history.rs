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

/// Byte offset in one message's UTF-8 text, not a bearer capability.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cursor {
    pub sequence: u64,
    pub offset: u64,
}

/// A stable log fence prevents a scan from chasing newly appended messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Read {
    pub session_id: String,
    pub through: Option<u64>,
    pub cursor: Option<Cursor>,
}
impl Read {
    pub fn validate(&self) -> Result<(), CommandError> {
        let invalid = || CommandError::Invalid("invalid history read".into());
        crate::name(&self.session_id).map_err(|_| invalid())?;
        const MAX: u64 = 9_007_199_254_740_991;
        if self.through.is_some_and(|value| value > MAX)
            || self.cursor.is_some_and(|cursor| {
                self.through.is_none() || cursor.sequence > MAX || cursor.offset > MAX
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Text visible in the durable conversation; reasoning and runtime metadata are
/// not conversation text. Tool JSON contributes string values, never its keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    ToolCall,
    ToolResult,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Chunk {
    pub message_id: String,
    pub turn_id: String,
    pub timestamp: u64,
    pub role: Role,
    pub sequence: u64,
    pub offset: u64,
    pub total_bytes: u64,
    pub text: String,
}

/// Index preparation is bounded too. Repeat with the returned fence and the
/// same cursor. Ready pages never silently truncate: next resumes exact bytes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Page {
    Preparing {
        through: u64,
    },
    Ready {
        through: u64,
        chunks: Vec<Chunk>,
        next: Option<Cursor>,
    },
}

/// Admitted Agent tools can recall this trusted Host profile's history.
/// Remote/background calls retain their actual principal's ReadHistory scope.
/// Every page rechecks current access and Session existence, including old fences.
pub trait Queries: Send + Sync {
    fn list(
        &self,
        call: Scope,
        input: super::catalog::List,
    ) -> BoxFuture<'_, Result<super::catalog::Page, CommandError>>;
    fn read(&self, call: Scope, input: Read) -> BoxFuture<'_, Result<Page, CommandError>>;
}
