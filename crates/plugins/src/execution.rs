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

use crate::{Error, name};
use maka_runtime::{event::Invocation, input::MessageInput};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
mod root;
pub use root::{CreateRoot, RootApproval, RootTemplate, Settings as RootSettings};

/// Persisted constraints, not a bearer capability. Only an explicit Host grant
/// binds them to a live plugin instance; current boundaries are still checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionBoundary {
    pub session_id: String,
    pub boundary_revision: u64,
    pub permission_mode: maka_runtime::execution::PermissionMode,
    pub cwd: String,
}
impl SessionBoundary {
    pub fn validate(&self) -> Result<(), Error> {
        name(&self.session_id)?;
        if self.boundary_revision >= (1 << 53)
            || self.cwd.is_empty()
            || self.cwd.len() > 32 * 1024
            || self.cwd.contains('\0')
        {
            return Err(Error::Invalid("invalid Session execution boundary".into()));
        }
        Ok(())
    }
}

/// Business identity belongs to the package/scope namespace, never an activation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Submit {
    pub operation_id: String,
    pub session_id: String,
    pub content: MessageInput,
    /// This execution only; never changes the Session's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration_mode: Option<maka_runtime::execution::BehaviorId>,
}

impl Submit {
    pub fn validate(&self) -> Result<(), Error> {
        name(&self.operation_id)?;
        name(&self.session_id)?;
        if self.content.text_bytes() > 64 * 1024 {
            return Err(Error::Invalid("execution message exceeds 64 KiB".into()));
        }
        maka_runtime::message::validate_sources(&self.content, &[])
            .map_err(|reason| Error::Invalid(reason.into()))
    }

    pub fn digest(&self) -> Result<String, Error> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| Error::Invalid(error.to_string()))?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

/// Acceptance is not completion. Outcomes remain in the Host's canonical log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub invocation: Invocation,
    pub message_id: String,
    pub content_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateChild {
    pub operation_id: String,
    pub parent_session_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<maka_runtime::execution::PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_tools: Option<std::collections::BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<ChildWorkspace>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildWorkspace {
    Inherit,
    IsolatedGit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Target {
    Model {
        model: maka_runtime::execution::ModelBinding,
        thinking_level: Option<maka_runtime::execution::ThinkingLevel>,
    },
    Executor {
        executor_id: maka_runtime::executor::ExecutorId,
    },
}
impl Target {
    pub fn validate(&self) -> Result<(), Error> {
        if let Self::Model { model, .. } = self {
            name(&model.connection_id)?;
            name(&model.connection_slug)?;
            name(&model.model)?;
        }
        Ok(())
    }
}

impl CreateChild {
    pub fn validate(&self) -> Result<(), Error> {
        name(&self.operation_id)?;
        name(&self.parent_session_id)?;
        if self.name.trim().is_empty()
            || self.name.len() > 256
            || self.name.chars().any(char::is_control)
        {
            return Err(Error::Invalid("invalid child Session name".into()));
        }
        if self
            .instructions
            .as_ref()
            .is_some_and(|text| text.len() > 16 * 1024)
            || self
                .bound_tools
                .as_ref()
                .is_some_and(|tools| tools.len() > 128)
        {
            return Err(Error::Invalid(
                "child Session surface exceeds its budget".into(),
            ));
        }
        if let Some(tools) = &self.bound_tools {
            for tool in tools {
                name(tool)?;
            }
        }
        if let Some(target) = &self.target {
            target.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChildSession {
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePatch {
    pub artifact_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub bytes: u64,
    pub base_commit: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Progress {
    Pending,
    Running,
    WaitingForUser,
    Paused,
    Ended {
        outcome: maka_runtime::event::InvocationOutcome,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub receipt: Receipt,
    pub progress: Progress,
    /// Exact log fence from the same snapshot as progress; use it for event pages.
    pub through_sequence: u64,
    /// Canonical terminal record from this same observation, absent while unfinished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_event_id: Option<String>,
    /// Stable identity of the current blocking interaction set or handoff pause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub events: Vec<maka_runtime::event::StoredEvent>,
    pub through_sequence: u64,
    pub next_after: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("plugin execution authority is retired or revoked")]
    Revoked,
    #[error("execution request is not authorized")]
    Denied,
    #[error("operation identity belongs to different content")]
    Conflict,
    #[error("execution operation was not accepted")]
    NotFound,
    #[error("Host is draining")]
    Draining,
    #[error("Session is busy")]
    Busy,
    #[error("execution acceptance outcome is unknown: {0}")]
    OutcomeUnknown(String),
    #[error("invalid execution request: {0}")]
    Invalid(String),
    #[error("Host execution failed: {0}")]
    Host(String),
}

/// A Host-authorized, instance-bound interface. Plugins cannot supply another
/// namespace or activation with a command; Host binds both when granting it.
pub trait Access: Send + Sync {
    /// Restore a Host-recorded consent reference, never plugin-supplied boundary data.
    fn restore(
        &self,
        id: crate::authorization::Id,
    ) -> futures_util::future::BoxFuture<'_, Result<std::sync::Arc<dyn Commands>, CommandError>>;
    /// Borrow a real Host call's execution authority. Entry placement and a
    /// caller-supplied Session identity never authorize execution on their own.
    fn acquire(
        &self,
        call: crate::call::Scope,
    ) -> futures_util::future::BoxFuture<'_, Result<std::sync::Arc<dyn Commands>, CommandError>>;
}

/// An acquired execution capability retains its captured permission ceiling.
pub trait Commands: Send + Sync {
    /// Current configuration of an authorized Session; not a grant or an
    /// unrestricted catalog. Returns no plugin-owned domain state.
    fn session(
        &self,
        session_id: String,
    ) -> futures_util::future::BoxFuture<'_, Result<crate::session::View, CommandError>>;
    /// Check current instance and Session boundaries without submitting work.
    fn validate_authority(&self) -> futures_util::future::BoxFuture<'_, Result<(), CommandError>>;
    fn create_root(
        &self,
        request: CreateRoot,
    ) -> futures_util::future::BoxFuture<'_, Result<ChildSession, CommandError>>;
    /// Host-captured constraints suitable for persisting with a business intent.
    fn boundaries(&self) -> Result<Vec<SessionBoundary>, CommandError>;
    /// Export a settled child workspace once; exact retries return its immutable Artifact.
    fn workspace_patch(
        &self,
        operation_id: String,
    ) -> futures_util::future::BoxFuture<'_, Result<Option<WorkspacePatch>, CommandError>>;
    fn event(
        &self,
        operation_id: String,
        event_id: String,
        through: u64,
    ) -> futures_util::future::BoxFuture<
        '_,
        Result<Option<maka_runtime::event::StoredEvent>, CommandError>,
    >;
    /// Invalidation only. The canonical query remains authoritative.
    fn changes(&self) -> Result<tokio::sync::watch::Receiver<u64>, CommandError>;
    fn events(
        &self,
        operation_id: String,
        after: u64,
        through: u64,
    ) -> futures_util::future::BoxFuture<'_, Result<EventPage, CommandError>>;
    fn create_child(
        &self,
        request: CreateChild,
    ) -> futures_util::future::BoxFuture<'_, Result<ChildSession, CommandError>>;
    fn submit(
        &self,
        request: Submit,
    ) -> futures_util::future::BoxFuture<'_, Result<Receipt, CommandError>>;
    fn query(
        &self,
        operation_id: String,
    ) -> futures_util::future::BoxFuture<'_, Result<Observation, CommandError>>;
    fn cancel(
        &self,
        operation_id: String,
    ) -> futures_util::future::BoxFuture<'_, Result<Observation, CommandError>>;
}
