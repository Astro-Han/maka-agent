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
use maka_runtime::attachment::AttachmentRef;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use maka_runtime::message::EditableMessage;
pub use maka_runtime::session::CopyPurpose;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopySource {
    pub session_id: String,
    pub expected_revision: u64,
    pub purpose: CopyPurpose,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopySession {
    pub source: CopySource,
    pub root: crate::execution::CreateRoot,
}
impl CopySession {
    pub fn validate(&self) -> Result<(), CommandError> {
        self.root
            .validate()
            .map_err(|error| CommandError::Invalid(error.to_string()))?;
        crate::name(&self.source.session_id)
            .map_err(|error| CommandError::Invalid(error.to_string()))?;
        if self.source.expected_revision == 0 || self.source.expected_revision >= 1 << 53 {
            return Err(CommandError::Invalid(
                "invalid source Session revision".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CopyResult {
    Committed {
        session: crate::execution::ChildSession,
    },
    SourceRevisionConflict {
        expected_revision: u64,
        actual_revision: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourcesRead {
    pub session_id: String,
    pub turn_id: String,
}
impl SourcesRead {
    pub fn validate(&self) -> Result<(), CommandError> {
        for id in [&self.session_id, &self.turn_id] {
            crate::name(id).map_err(|_| CommandError::Invalid("invalid message source".into()))?;
        }
        Ok(())
    }
}

/// Copy an uploaded historical material into an independently authorized Session.
/// Identifiers locate content; the history call and destination capability grant access.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopyMaterial {
    pub session_id: String,
    pub artifact_id: String,
    pub target_session_id: String,
}
impl CopyMaterial {
    pub fn validate(&self) -> Result<(), CommandError> {
        for id in [&self.session_id, &self.artifact_id, &self.target_session_id] {
            crate::name(id)
                .map_err(|_| CommandError::Invalid("invalid history material".into()))?;
        }
        Ok(())
    }
}

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
    /// User attachment descriptors, present only on the first chunk of a message.
    pub attachments: Vec<AttachmentRef>,
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
pub trait History: Send + Sync {
    /// A new root with owned immutable history. Source read access does not grant
    /// destination creation; both capabilities and the source workspace are checked.
    fn copy_session(
        &self,
        call: Scope,
        target: Arc<dyn crate::execution::Commands>,
        input: CopySession,
    ) -> BoxFuture<'_, Result<CopyResult, CommandError>>;
    /// Ordered preparation-free opening sources of a Turn, not its aggregated UI row.
    /// Their identities and intent grant no execution authority.
    fn sources(
        &self,
        call: Scope,
        input: SourcesRead,
    ) -> BoxFuture<'_, Result<Vec<EditableMessage>, CommandError>>;
    fn list(
        &self,
        call: Scope,
        input: super::catalog::List,
    ) -> BoxFuture<'_, Result<super::catalog::Page, CommandError>>;
    fn read(&self, call: Scope, input: Read) -> BoxFuture<'_, Result<Page, CommandError>>;
    fn copy_material(
        &self,
        call: Scope,
        target: Arc<dyn crate::execution::Commands>,
        input: CopyMaterial,
    ) -> BoxFuture<'_, Result<AttachmentRef, CommandError>>;
}
