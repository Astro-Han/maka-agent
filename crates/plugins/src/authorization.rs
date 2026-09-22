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

//! Consent descriptions and durable references, never bearer capabilities.

use maka_runtime::execution::{SandboxMode, WorkspaceTarget};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Resolving a reference requires the original package/scope, a live activation,
/// an unrevoked Host record and the issuing principal's current authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ReadFiles,
    WriteFiles,
    Network,
    Models,
    Processes,
    ClientCapabilities,
    Executions,
    Notifications,
    ReadSessions,
    ReadHistory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Target {
    Profile,
    /// Host-created scratch workspace for this package/scope, never arbitrary state files.
    PluginWorkspace {
        sandbox_mode: SandboxMode,
    },
    Directory {
        path: String,
    },
    Session {
        session_id: String,
    },
    Workspace {
        workspace: WorkspaceTarget,
        sandbox_mode: SandboxMode,
    },
}

/// A proposal is inert. Only the authenticated Host consent path may approve it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    /// Stable across a lost reply; a different proposal needs a new identity.
    pub operation_id: Uuid,
    pub title: String,
    pub target: Target,
    pub capabilities: BTreeSet<Capability>,
}
impl Request {
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.title.trim().is_empty()
            || self.title.len() > 256
            || self.title.chars().any(char::is_control)
            || self.capabilities.is_empty()
        {
            return Err(crate::Error::Invalid(
                "invalid authorization description".into(),
            ));
        }
        match &self.target {
            Target::Profile
                if self.capabilities.iter().all(|capability| {
                    matches!(
                        capability,
                        Capability::Notifications
                            | Capability::ReadSessions
                            | Capability::ReadHistory
                    )
                }) => {}
            Target::Profile => {
                return Err(crate::Error::Invalid(
                    "profile authorization has no workspace or execution target".into(),
                ));
            }
            Target::Session { session_id } => crate::name(session_id)?,
            Target::PluginWorkspace { sandbox_mode } => self.validate_mode(*sandbox_mode)?,
            Target::Directory { path } => {
                if path.is_empty()
                    || path.len() > 32 * 1024
                    || path.contains('\0')
                    || self.capabilities.iter().any(|capability| {
                        !matches!(capability, Capability::ReadFiles | Capability::WriteFiles)
                    })
                {
                    return Err(crate::Error::Invalid(
                        "directory consent only authorizes file access".into(),
                    ));
                }
            }
            Target::Workspace {
                workspace,
                sandbox_mode,
            } => {
                match workspace {
                    WorkspaceTarget::Project { project_id } => crate::name(project_id)?,
                    WorkspaceTarget::HostPath { path }
                        if !path.is_empty() && path.len() <= 32 * 1024 && !path.contains('\0') => {}
                    _ => {
                        return Err(crate::Error::Invalid(
                            "invalid authorization workspace".into(),
                        ));
                    }
                }
                self.validate_mode(*sandbox_mode)?;
            }
        }
        Ok(())
    }
    /// Explicit HTTP consent is independent of the file/process sandbox. Client
    /// capabilities can cross that sandbox and still require unrestricted access.
    pub fn validate_mode(&self, mode: SandboxMode) -> Result<(), crate::Error> {
        if mode != SandboxMode::DangerFullAccess
            && self.capabilities.contains(&Capability::ClientCapabilities)
        {
            return Err(crate::Error::Invalid(
                "client capabilities require unrestricted sandbox permission".into(),
            ));
        }
        if mode == SandboxMode::ReadOnly && self.capabilities.contains(&Capability::WriteFiles) {
            return Err(crate::Error::Invalid(
                "read-only authorization cannot grant file writes".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grant {
    pub id: Id,
    pub request: Request,
    pub revoked: bool,
}

pub struct Authorized {
    /// Observation of the consent being used, not a second source of authority.
    pub grant: Grant,
    /// Host-resolved observation: aliases in the proposal are not canonical paths.
    /// This value cannot be used to mint or widen authority.
    pub boundary: Boundary,
    pub call: crate::call::Owned,
}

pub trait Access: Send + Sync {
    /// Restore current authority, not a serialized in-memory capability. Each
    /// resource operation must still check revocation and current policy.
    fn open(
        &self,
        id: Id,
    ) -> futures_util::future::BoxFuture<'_, Result<Authorized, crate::execution::CommandError>>;
}

/// Frozen observation, not a capability. Only a Host-issued Scope can use it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Boundary {
    Profile,
    Directory {
        path: String,
        identity: maka_runtime::execution::DirectoryIdentity,
    },
    Session {
        boundary: crate::execution::SessionBoundary,
        workspace_identity: maka_runtime::execution::WorkspaceIdentity,
    },
    Workspace {
        workspace: maka_runtime::execution::WorkspaceProjection,
        workspace_identity: maka_runtime::execution::WorkspaceIdentity,
        origin: maka_runtime::execution::WorkspaceOrigin,
        sandbox_mode: SandboxMode,
    },
}
impl Boundary {
    pub fn validate(&self, request: &Request) -> Result<(), crate::Error> {
        let invalid =
            || crate::Error::Invalid("authorization boundary does not match proposal".into());
        let mode = match (self, &request.target) {
            (Self::Profile, Target::Profile) => SandboxMode::ReadOnly,
            (Self::Directory { path, .. }, Target::Directory { .. }) if !path.is_empty() => {
                SandboxMode::DangerFullAccess
            }
            (
                Self::Workspace {
                    workspace,
                    sandbox_mode,
                    origin: maka_runtime::execution::WorkspaceOrigin::Allocated,
                    ..
                },
                Target::PluginWorkspace {
                    sandbox_mode: proposed,
                },
            ) if sandbox_mode == proposed
                && !workspace.host_cwd.is_empty()
                && matches!(&workspace.target, WorkspaceTarget::HostPath { path } if path == &workspace.host_cwd) =>
            {
                *sandbox_mode
            }
            (Self::Session { boundary, .. }, Target::Session { session_id })
                if &boundary.session_id == session_id =>
            {
                boundary.validate().map_err(|_| invalid())?;
                boundary.sandbox_mode
            }
            (
                Self::Workspace {
                    workspace,
                    sandbox_mode,
                    origin: maka_runtime::execution::WorkspaceOrigin::Selected,
                    ..
                },
                Target::Workspace {
                    workspace: target,
                    sandbox_mode: proposed,
                },
            ) if sandbox_mode == proposed
                && match (&workspace.target, target) {
                    (
                        WorkspaceTarget::Project { project_id: left },
                        WorkspaceTarget::Project { project_id: right },
                    ) => left == right,
                    // Only Host resolves path aliases into this canonical snapshot.
                    (WorkspaceTarget::HostPath { .. }, WorkspaceTarget::HostPath { .. }) => true,
                    _ => false,
                }
                && !workspace.host_cwd.is_empty() =>
            {
                *sandbox_mode
            }
            _ => return Err(invalid()),
        };
        request.validate_mode(mode)
    }
}
