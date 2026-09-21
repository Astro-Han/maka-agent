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

use maka_runtime::execution::{PermissionMode, WorkspaceTarget};
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
        permission_mode: PermissionMode,
    },
    Directory {
        path: String,
    },
    Session {
        session_id: String,
    },
    Workspace {
        workspace: WorkspaceTarget,
        permission_mode: PermissionMode,
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
            Target::PluginWorkspace { permission_mode } => self.validate_mode(*permission_mode)?,
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
                permission_mode,
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
                self.validate_mode(*permission_mode)?;
            }
        }
        Ok(())
    }
    /// Interactive approvals remain a separate Host operation. An unattended
    /// grant cannot turn Ask/Explore into permission for arbitrary side effects.
    pub fn validate_mode(&self, mode: PermissionMode) -> Result<(), crate::Error> {
        if mode != PermissionMode::Bypass
            && self.capabilities.iter().any(|capability| {
                matches!(
                    capability,
                    Capability::WriteFiles
                        | Capability::Network
                        | Capability::Processes
                        | Capability::ClientCapabilities
                )
            })
        {
            return Err(crate::Error::Invalid(
                "unattended side effects require explicit bypass permission".into(),
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
        permission_mode: PermissionMode,
    },
}
impl Boundary {
    pub fn validate(&self, request: &Request) -> Result<(), crate::Error> {
        let invalid =
            || crate::Error::Invalid("authorization boundary does not match proposal".into());
        let mode = match (self, &request.target) {
            (Self::Profile, Target::Profile) => PermissionMode::Explore,
            (Self::Directory { path, .. }, Target::Directory { .. }) if !path.is_empty() => {
                PermissionMode::Bypass
            }
            (
                Self::Workspace {
                    workspace,
                    permission_mode,
                    ..
                },
                Target::PluginWorkspace {
                    permission_mode: proposed,
                },
            ) if permission_mode == proposed
                && !workspace.host_cwd.is_empty()
                && matches!(&workspace.target, WorkspaceTarget::HostPath { path } if path == &workspace.host_cwd) =>
            {
                *permission_mode
            }
            (Self::Session { boundary, .. }, Target::Session { session_id })
                if &boundary.session_id == session_id =>
            {
                boundary.validate().map_err(|_| invalid())?;
                boundary.permission_mode
            }
            (
                Self::Workspace {
                    workspace,
                    permission_mode,
                    ..
                },
                Target::Workspace {
                    workspace: target,
                    permission_mode: proposed,
                },
            ) if permission_mode == proposed
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
                *permission_mode
            }
            _ => return Err(invalid()),
        };
        request.validate_mode(mode)
    }
}
