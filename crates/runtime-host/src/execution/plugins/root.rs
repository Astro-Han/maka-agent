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

use super::{BoundCommands, ChildSession, Error, Executions, Grant, SessionConfiguration, storage};
use crate::session::PreparedSession;
use maka_plugins::{
    authorization::Boundary,
    execution::{CreateRoot, RootApproval, SessionBoundary, Target},
};
use maka_protocol::session::{
    SessionCreateInput, SessionCreateTarget, SessionModelTarget, WorkspaceProjection,
};
use maka_runtime::execution::{SandboxMode, WorkspaceIdentity, WorkspaceTarget};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, serde::Serialize)]
pub(super) struct RootGrant {
    pub workspace_origin: maka_runtime::execution::WorkspaceOrigin,
    pub workspace: WorkspaceProjection,
    pub workspace_identity: WorkspaceIdentity,
    pub sandbox_mode: SandboxMode,
    pub approval_policy: maka_runtime::execution::ApprovalPolicy,
    pub source: Option<SessionBoundary>,
}
impl From<RootApproval> for RootGrant {
    fn from(approval: RootApproval) -> Self {
        Self {
            workspace_origin: approval.source.as_ref().map_or(
                maka_runtime::execution::WorkspaceOrigin::Selected,
                |source| source.workspace_origin,
            ),
            workspace: WorkspaceProjection {
                target: approval.template.workspace,
                host_cwd: approval.template.cwd,
            },
            workspace_identity: approval.template.workspace_identity,
            sandbox_mode: approval.template.sandbox_mode,
            approval_policy: approval.template.approval_policy,
            source: approval.source,
        }
    }
}

impl BoundCommands {
    pub(super) async fn authorize_origin(&self, host: &Executions) -> Result<(), Error> {
        if let Some(call) = &self.call {
            match host.plugin_execution_boundary(call).await? {
                Boundary::Session { boundary, .. } => {
                    let grants = self.grants.lock().unwrap();
                    let grant = grants.get(&boundary.session_id).ok_or(Error::Denied)?;
                    if grant.boundary_revision != boundary.boundary_revision
                        || grant.sandbox_mode != boundary.sandbox_mode
                        || grant.approval_policy != boundary.approval_policy
                        || grant.cwd != boundary.cwd
                        || grant.workspace_origin != boundary.workspace_origin
                    {
                        return Err(Error::Denied);
                    }
                }
                Boundary::Workspace {
                    workspace,
                    workspace_identity,
                    origin,
                    sandbox_mode,
                } => {
                    let grant = self.root_grant.as_ref().ok_or(Error::Denied)?;
                    if grant.workspace != workspace
                        || grant.workspace_origin != origin
                        || grant.workspace_identity != workspace_identity
                        || grant.sandbox_mode != sandbox_mode
                    {
                        return Err(Error::Denied);
                    }
                }
                Boundary::Profile | Boundary::Directory { .. } => return Err(Error::Denied),
            }
        }
        if let Some(id) = self.consent {
            crate::server::plugin_authorization::validate(
                &host.log,
                &host.configuration,
                &self.namespace,
                id,
                maka_plugins::authorization::Capability::Executions,
            )
            .await
            .map_err(super::authority::consent_error)?;
        }
        let Some(source) = self
            .root_grant
            .as_ref()
            .and_then(|root| root.source.as_ref())
        else {
            return Ok(());
        };
        if host
            .log
            .session_manager(&source.session_id)
            .await
            .map_err(storage)?
            .is_some_and(|manager| manager != self.namespace)
        {
            return Err(Error::Denied);
        }
        let current = host
            .log
            .get_session::<SessionConfiguration>(&source.session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        if current.archived
            || current.configuration.boundary_revision != source.boundary_revision
            || current.configuration.sandbox_mode != source.sandbox_mode
            || current.configuration.approval_policy != source.approval_policy
            || current.configuration.workspace.host_cwd != source.cwd
            || current.configuration.workspace_origin != source.workspace_origin
        {
            return Err(Error::Denied);
        }
        Ok(())
    }
    pub(super) async fn root(&self, request: CreateRoot) -> Result<ChildSession, Error> {
        request
            .validate()
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let approval = self.root_grant.as_ref().ok_or(Error::Denied)?;
        if rank(request.settings.sandbox_mode) > rank(approval.sandbox_mode)
            || !request
                .settings
                .approval_policy
                .is_subset_of(approval.approval_policy)
        {
            return Err(Error::Denied);
        }
        let host = self.executions()?;
        let lease = self.context.admit().map_err(|_| Error::Revoked)?;
        self.authorize_origin(&host).await?;
        let bytes = serde_json::to_vec(&(
            "plugin-root-v1",
            self.namespace.package(),
            String::from(self.namespace.scope().clone()),
            &request.operation_id,
        ))
        .map_err(|error| Error::Invalid(error.to_string()))?;
        let id = format!("plugin-root-{:x}", Sha256::digest(bytes));
        let fingerprint = format!(
            "sha256:{:x}",
            Sha256::digest(
                serde_json::to_vec(&(approval, &request))
                    .map_err(|error| Error::Invalid(error.to_string()))?
            )
        );
        let observed_project = match &approval.workspace.target {
            WorkspaceTarget::Project { project_id } => Some(
                host.log
                    .get_project(project_id)
                    .await
                    .map_err(storage)?
                    .ok_or(Error::NotFound)?,
            ),
            WorkspaceTarget::HostPath { .. } => None,
        };
        // Filesystem observation never holds Host-wide admission.
        if let Some(project) = &observed_project {
            let resolved = crate::server::resolve_project_workspace(project.clone())
                .await
                .map_err(|error| Error::Invalid(error.message))?;
            if resolved.host_cwd != approval.workspace.host_cwd {
                return Err(Error::Denied);
            }
        }
        let path = std::path::PathBuf::from(&approval.workspace.host_cwd);
        let expected = approval.workspace_identity.clone();
        tokio::task::spawn_blocking(move || {
            let canonical = path
                .canonicalize()
                .map_err(|error| Error::Host(error.to_string()))?;
            if maka_fs_tools::workspace::project::host_path(&canonical)
                .map_err(|error| Error::Host(error.to_string()))?
                != path.to_str().ok_or(Error::Denied)?
                || maka_fs_tools::workspace::read_identity(&canonical)
                    .map_err(|error| Error::Host(error.to_string()))?
                    != expected
            {
                return Err(Error::Denied);
            }
            Ok(())
        })
        .await
        .map_err(|error| Error::Host(error.to_string()))??;
        let gate = host.interactions.own_admission().await;
        self.authorize_origin(&host).await?;
        if let Some(project) = observed_project
            && host
                .log
                .get_project(&project.id)
                .await
                .map_err(storage)?
                .as_ref()
                != Some(&project)
        {
            return Err(Error::Conflict);
        }
        let approval = approval.clone();
        let worker = host.clone();
        let grants = self.grants.clone();
        let namespace = self.namespace.clone();
        let submission_stop = self.submission_stop.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        host.workers.spawn(async move {
            let result = async {
                if !worker.accepting() {
                    return Err(Error::Draining);
                }
                let existing = worker
                    .log
                    .probe_session_create::<SessionConfiguration>(&id, &fingerprint)
                    .await
                    .map_err(storage)?;
                let expected =
                    configuration(&worker, &id, &request, &approval, existing.is_none()).await?;
                let record = match existing {
                    Some(record) => record,
                    None => {
                        if submission_stop.is_cancelled() {
                            return Err(Error::Revoked);
                        }
                        let record = if request.managed {
                            worker
                                .log
                                .create_managed_session(
                                    &maka_event_log::sessions::ManagedSession {
                                        session_id: id.clone(),
                                        manager: namespace.clone(),
                                        fingerprint: fingerprint.clone(),
                                    },
                                    &expected,
                                    now()?,
                                )
                                .await
                        } else {
                            worker
                                .log
                                .create_session(&id, &fingerprint, &expected, now()?)
                                .await
                        }
                        .map_err(storage)?;
                        if worker.catalog.publish_session(&id).await.is_err() {
                            worker.begin_drain();
                        }
                        record
                    }
                };
                let manager = worker.log.session_manager(&id).await.map_err(storage)?;
                if manager.as_ref() != request.managed.then_some(&namespace) {
                    return Err(Error::Denied);
                }
                let current = &record.configuration;
                if record.archived
                    || current.boundary_revision != 0
                    || current.workspace != expected.workspace
                    || current.workspace_origin != expected.workspace_origin
                    || current.sandbox_mode != expected.sandbox_mode
                    || current.approval_policy != expected.approval_policy
                    || current.collaboration_mode != expected.collaboration_mode
                    || current.orchestration_mode != expected.orchestration_mode
                    || current.instructions != expected.instructions
                    || current.tool_profile != expected.tool_profile
                    || current.bound_tools != expected.bound_tools
                {
                    return Err(Error::Denied);
                }
                grants.lock().unwrap().insert(
                    id.clone(),
                    Grant {
                        workspace_origin: record.configuration.workspace_origin,
                        boundary_revision: 0,
                        sandbox_mode: expected.sandbox_mode,
                        approval_policy: expected.approval_policy,
                        cwd: expected.workspace.host_cwd,
                    },
                );
                Ok(ChildSession { session_id: id })
            }
            .await;
            if matches!(result, Err(Error::OutcomeUnknown(_))) {
                worker.begin_drain();
            }
            drop(gate);
            drop(lease);
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| Error::OutcomeUnknown("root Session owner disappeared".into()))?
    }
}

async fn configuration(
    host: &Arc<Executions>,
    id: &str,
    request: &CreateRoot,
    approval: &RootGrant,
    resolve_target: bool,
) -> Result<SessionConfiguration, Error> {
    let settings = &request.settings;
    let (target, bound, thinking_level) = match &settings.target {
        Target::Model {
            model,
            thinking_level,
        } => {
            let target = SessionModelTarget::Explicit {
                connection_id: model.connection_id.clone(),
                connection_slug: model.connection_slug.clone(),
                model: model.model.clone(),
            };
            let bound = if resolve_target {
                crate::session::model::resolve(&host.configuration, &target, *thinking_level)
                    .await
                    .map_err(|error| Error::Invalid(error.message))?
            } else {
                model.clone()
            };
            (
                SessionCreateTarget::Model {
                    model_target: target,
                },
                bound.into(),
                *thinking_level,
            )
        }
        Target::Executor {
            executor_id,
            settings,
        } => {
            if resolve_target {
                host.executor_binding(id, executor_id)
                    .map_err(|error| Error::Invalid(error.message))?;
            }
            (
                SessionCreateTarget::Executor {
                    executor_id: executor_id.clone(),
                    executor_settings: settings.clone(),
                },
                crate::session::SessionTarget::Executor {
                    executor_id: executor_id.clone(),
                    settings: settings.clone(),
                },
                None,
            )
        }
    };
    let prepared = PreparedSession::new(SessionCreateInput {
        session_id: id.into(),
        workspace: approval.workspace.target.clone(),
        target,
        mode: None,
        name: Some(request.name.clone()),
        labels: None,
        thinking_level,
        tool_profile: None,
        // The explicit Host grant supplies the default to bind below. The
        // interactive create codec restricts Explore to UI-specific modes.
        sandbox_mode: None,
        approval_policy: Some(settings.approval_policy),
        collaboration_mode: Some(settings.collaboration_mode),
        orchestration_mode: Some(settings.behavior.clone()),
    })
    .map_err(|error| Error::Invalid(error.to_string()))?;
    let mut config = prepared.bind(approval.workspace.clone(), bound, settings.sandbox_mode);
    config.bound_tools = settings.bound_tools.clone();
    config.workspace_origin = approval.workspace_origin;
    config.instructions = settings.instructions.clone();
    if let Some(source) = &approval.source {
        if rank(settings.sandbox_mode) > rank(source.sandbox_mode)
            || !settings
                .approval_policy
                .is_subset_of(source.approval_policy)
            || approval.workspace.host_cwd != source.cwd
        {
            return Err(Error::Denied);
        }
        let origin = host
            .log
            .get_session::<SessionConfiguration>(&source.session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        if let Some(ceiling) = origin.configuration.bound_tools {
            config.bound_tools = Some(match config.bound_tools {
                Some(selected) => selected.intersection(&ceiling).cloned().collect(),
                None => ceiling,
            });
        }
        config.tool_profile = origin.configuration.tool_profile;
        config.instructions = match (origin.configuration.instructions, config.instructions) {
            (Some(base), Some(extra)) => Some(format!("{base}\n\n{extra}")),
            (base, extra) => base.or(extra),
        };
        if config
            .instructions
            .as_ref()
            .is_some_and(|text| text.len() > 16 * 1024)
        {
            return Err(Error::Invalid("combined instructions exceed 16 KiB".into()));
        }
    }
    Ok(config)
}
fn rank(mode: SandboxMode) -> u8 {
    match mode {
        SandboxMode::ReadOnly => 0,
        SandboxMode::WorkspaceWrite => 1,
        SandboxMode::DangerFullAccess => 2,
    }
}
fn now() -> Result<u64, Error> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| Error::Host(error.to_string()))?
        .as_millis()
        .try_into()
        .map_err(|_| Error::Host("clock overflow".into()))
}
