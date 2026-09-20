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

use super::{
    BoundCommands, Commands, Context, Error, Executions, Grant, RootGrant, Scope,
    SessionConfiguration, storage,
};
use maka_plugins::{
    authorization::Boundary,
    execution::{RootApproval, SessionBoundary},
    storage::Namespace,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

impl Executions {
    pub(crate) async fn acquire_plugin_execution(
        self: &Arc<Self>,
        context: Context,
        call: maka_plugins::call::Scope,
        root_id: &str,
    ) -> Result<Arc<dyn Commands>, Error> {
        let _lease = context.admit().map_err(|_| Error::Revoked)?;
        let boundary = self.plugin_execution_boundary(&call).await?;
        let mut commands =
            self.bind_plugin_boundary(context, boundary, root_id, call.cancellation.clone())?;
        commands.call = Some(call);
        Ok(Arc::new(commands))
    }

    pub(super) async fn plugin_execution_boundary(
        &self,
        call: &maka_plugins::call::Scope,
    ) -> Result<Boundary, Error> {
        if !self.accepting() || !self.plugin_calls.owns(call) || call.cancellation.is_cancelled() {
            return Err(Error::Revoked);
        }
        let Some(invocation) = call.identity.agent() else {
            return self
                .plugin_resource_boundary(call, maka_plugins::authorization::Capability::Executions)
                .await;
        };
        let frozen = self
            .log
            .invocation_configuration(invocation)
            .await
            .map_err(storage)?
            .ok_or(Error::Denied)?;
        let current = self
            .log
            .get_session::<SessionConfiguration>(&invocation.session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        if current.archived
            || current.configuration.workspace.host_cwd != frozen.cwd
            || current.configuration.permission_mode != frozen.permission_mode
        {
            return Err(Error::Denied);
        }
        let cwd = frozen.cwd.clone();
        let workspace_identity = tokio::task::spawn_blocking(move || {
            let observed = maka_fs_tools::workspace::read_identity(std::path::Path::new(&cwd))
                .map_err(|_| Error::Denied)?;
            if frozen.workspace_identity.as_ref() != Some(&observed) {
                return Err(Error::Denied);
            }
            Ok(observed)
        })
        .await
        .map_err(|error| Error::Host(error.to_string()))??;
        Ok(Boundary::Session {
            boundary: SessionBoundary {
                session_id: invocation.session_id.clone(),
                boundary_revision: current.configuration.boundary_revision,
                permission_mode: current.configuration.permission_mode,
                cwd: current.configuration.workspace.host_cwd,
            },
            workspace_identity,
        })
    }

    pub(crate) async fn restore_plugin_consent(
        self: &Arc<Self>,
        context: Context,
        id: maka_plugins::authorization::Id,
        root_id: &str,
    ) -> Result<Arc<dyn Commands>, Error> {
        let identity = context.identity().map_err(|_| Error::Revoked)?;
        let namespace = Namespace::new(identity.package_id, identity.scope)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let record = crate::server::plugin_authorization::validate(
            &self.log,
            &self.configuration,
            &namespace,
            id,
            maka_plugins::authorization::Capability::Executions,
        )
        .await
        .map_err(consent_error)?;
        let mut commands = self.bind_plugin_boundary(
            context.clone(),
            record.boundary,
            root_id,
            context.stopping().map_err(|_| Error::Revoked)?,
        )?;
        commands.consent = Some(id);
        Ok(Arc::new(commands))
    }
    pub(crate) async fn authorize_plugin(
        self: &Arc<Self>,
        context: Context,
        sessions: &[String],
        root_id: &str,
        submission_stop: CancellationToken,
    ) -> Result<Arc<dyn Commands>, Error> {
        if sessions.len() > 256 {
            return Err(Error::Denied);
        }
        let mut boundaries = Vec::with_capacity(sessions.len());
        for id in sessions {
            let session = self
                .log
                .get_session::<SessionConfiguration>(id)
                .await
                .map_err(storage)?
                .ok_or(Error::NotFound)?;
            if session.archived {
                return Err(Error::Denied);
            }
            boundaries.push(SessionBoundary {
                session_id: id.clone(),
                boundary_revision: session.configuration.boundary_revision,
                permission_mode: session.configuration.permission_mode,
                cwd: session.configuration.workspace.host_cwd,
            });
        }
        self.restore_plugin_authority(context, boundaries, root_id, submission_stop)
    }

    /// Explicit Host grant constrained by its original persisted boundary, not
    /// reinterpreted using current (possibly broader) Session configuration.
    pub(crate) fn restore_plugin_authority(
        self: &Arc<Self>,
        context: Context,
        boundaries: Vec<SessionBoundary>,
        root_id: &str,
        submission_stop: CancellationToken,
    ) -> Result<Arc<dyn Commands>, Error> {
        Ok(Arc::new(self.bind_plugin_authority(
            context,
            boundaries,
            None,
            root_id,
            submission_stop,
        )?))
    }

    pub(crate) fn authorize_plugin_root(
        self: &Arc<Self>,
        context: Context,
        approval: RootApproval,
        root_id: &str,
        submission_stop: CancellationToken,
    ) -> Result<Arc<dyn Commands>, Error> {
        approval
            .template
            .validate()
            .map_err(|error| Error::Invalid(error.to_string()))?;
        if let Some(source) = &approval.source {
            source
                .validate()
                .map_err(|error| Error::Invalid(error.to_string()))?;
        }
        Ok(Arc::new(self.bind_plugin_authority(
            context,
            Vec::new(),
            Some(RootGrant::from(approval)),
            root_id,
            submission_stop,
        )?))
    }

    fn bind_plugin_boundary(
        self: &Arc<Self>,
        context: Context,
        boundary: Boundary,
        root_id: &str,
        submission_stop: CancellationToken,
    ) -> Result<BoundCommands, Error> {
        let (sessions, root) = match boundary {
            Boundary::Session { boundary, .. } => (vec![boundary], None),
            Boundary::Workspace {
                workspace,
                workspace_identity,
                permission_mode,
            } => (
                Vec::new(),
                Some(RootGrant {
                    workspace,
                    workspace_identity,
                    permission_mode,
                    source: None,
                }),
            ),
            Boundary::Profile => return Err(Error::Denied),
        };
        self.bind_plugin_authority(context, sessions, root, root_id, submission_stop)
    }

    fn bind_plugin_authority(
        self: &Arc<Self>,
        context: Context,
        boundaries: Vec<SessionBoundary>,
        root_grant: Option<RootGrant>,
        root_id: &str,
        submission_stop: CancellationToken,
    ) -> Result<BoundCommands, Error> {
        let identity = context.identity().map_err(|_| Error::Revoked)?;
        if boundaries.len() > 256 || matches!(identity.scope, Scope::DesktopUi) {
            return Err(Error::Denied);
        }
        if let Scope::Session(scope) = &identity.scope
            && root_grant.as_ref().is_some_and(|root| {
                root.source
                    .as_ref()
                    .is_none_or(|source| &source.session_id != scope)
            })
        {
            return Err(Error::Denied);
        }
        let mut grants = BTreeMap::new();
        for boundary in boundaries {
            boundary
                .validate()
                .map_err(|error| Error::Invalid(error.to_string()))?;
            if let Scope::Session(scope) = &identity.scope
                && scope != &boundary.session_id
            {
                return Err(Error::Denied);
            }
            let grant = Grant {
                boundary_revision: boundary.boundary_revision,
                permission_mode: boundary.permission_mode,
                cwd: boundary.cwd,
            };
            if grants.insert(boundary.session_id, grant).is_some() {
                return Err(Error::Invalid("duplicate Session authorization".into()));
            }
        }
        Ok(BoundCommands {
            executions: Arc::downgrade(self),
            namespace: Namespace::new(identity.package_id, identity.scope)
                .map_err(|error| Error::Invalid(error.to_string()))?,
            context,
            grants: Arc::new(Mutex::new(grants)),
            root_id: root_id.into(),
            submission_stop,
            root_grant,
            consent: None,
            call: None,
        })
    }
}

pub(super) fn consent_error(error: maka_protocol::OperationError) -> Error {
    match error.code {
        maka_protocol::OperationErrorCode::Unauthorized => Error::Revoked,
        maka_protocol::OperationErrorCode::CommitOutcomeUnknown => {
            Error::OutcomeUnknown(error.message)
        }
        _ => Error::Host(error.message),
    }
}
