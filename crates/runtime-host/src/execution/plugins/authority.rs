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
    BoundCommands, Commands, Context, Error, Executions, Grant, Scope, SessionConfiguration,
    storage,
};
use maka_plugins::{
    execution::{RootApproval, SessionBoundary},
    storage::Namespace,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

impl Executions {
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
        self.bind_plugin_authority(context, boundaries, None, root_id, submission_stop)
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
        self.bind_plugin_authority(
            context,
            Vec::new(),
            Some(approval),
            root_id,
            submission_stop,
        )
    }

    fn bind_plugin_authority(
        self: &Arc<Self>,
        context: Context,
        boundaries: Vec<SessionBoundary>,
        root_approval: Option<RootApproval>,
        root_id: &str,
        submission_stop: CancellationToken,
    ) -> Result<Arc<dyn Commands>, Error> {
        let identity = context.identity().map_err(|_| Error::Revoked)?;
        if boundaries.len() > 256 || matches!(identity.scope, Scope::DesktopUi) {
            return Err(Error::Denied);
        }
        if let Scope::Session(scope) = &identity.scope
            && root_approval.as_ref().is_some_and(|root| {
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
        Ok(Arc::new(BoundCommands {
            executions: Arc::downgrade(self),
            namespace: Namespace::new(identity.package_id, identity.scope)
                .map_err(|error| Error::Invalid(error.to_string()))?,
            context,
            grants: Arc::new(Mutex::new(grants)),
            root_id: root_id.into(),
            submission_stop,
            root_approval,
        }))
    }
}
