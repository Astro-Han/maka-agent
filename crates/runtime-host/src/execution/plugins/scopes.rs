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

use super::{Error, Executions};
use crate::server::plugin_authorization as consent;
use maka_config::plugin_authorization::{Boundary, Principal};
use maka_plugins::{
    authorization::{Capability, Id, Request},
    call::{Identity, Owned, Scope},
    fiber::Context,
    storage::Namespace,
};
use tokio_util::sync::CancellationToken;

// Private admission evidence, carried unchanged through Service forwarding.
// Keeping it on the scope avoids a second global table of ephemeral grants.
struct Evidence {
    owner: Context,
    namespace: Namespace,
    boundary: Boundary,
    source: Source,
    clients: std::sync::OnceLock<Vec<std::sync::Arc<maka_client_capability::Registration>>>,
}
enum Source {
    Background(Id),
    Remote {
        principal: Principal,
        request: Request,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ResourceTarget {
    Session(String),
    Workspace(String),
}
impl ResourceTarget {
    pub fn session(&self) -> Option<&str> {
        match self {
            Self::Session(id) => Some(id),
            Self::Workspace(_) => None,
        }
    }
}

impl Executions {
    pub(crate) async fn open_plugin_consent(&self, owner: Context, id: Id) -> Result<Owned, Error> {
        let _lease = owner.admit().map_err(|_| Error::Revoked)?;
        let identity = owner.identity().map_err(|_| Error::Revoked)?;
        let namespace =
            Namespace::new(identity.package_id, identity.scope).map_err(|_| Error::Denied)?;
        let record = consent::restore(&self.log, &self.configuration, &namespace, id)
            .await
            .map_err(super::authority::consent_error)?;
        let cancellation = owner.stopping().map_err(|_| Error::Revoked)?;
        let scope = self
            .plugin_calls
            .issue_with(
                Identity::Background { grant: id },
                Evidence {
                    owner,
                    namespace,
                    boundary: record.boundary,
                    source: Source::Background(id),
                    clients: Default::default(),
                },
                cancellation,
            )
            .map_err(|error| Error::Host(error.to_string()))?;
        self.track_plugin_scope(scope, None)
    }

    pub(crate) async fn open_plugin_remote(
        &self,
        owner: Context,
        principal: Principal,
        request: Request,
        cancellation: CancellationToken,
        resources: std::sync::Arc<maka_plugins::call::Resources>,
    ) -> Result<Owned, Error> {
        let _lease = owner.admit().map_err(|_| Error::Revoked)?;
        let identity = owner.identity().map_err(|_| Error::Revoked)?;
        let namespace =
            Namespace::new(identity.package_id, identity.scope).map_err(|_| Error::Denied)?;
        request
            .validate()
            .map_err(|error| Error::Invalid(error.to_string()))?;
        consent::validate_principal(&self.configuration, &principal, &request)
            .await
            .map_err(super::authority::consent_error)?;
        let boundary = consent::capture(&self.log, &request)
            .await
            .map_err(super::authority::consent_error)?;
        let scope = self
            .plugin_calls
            .issue_with(
                Identity::Remote {
                    request_id: request.operation_id,
                },
                Evidence {
                    owner,
                    namespace,
                    boundary,
                    source: Source::Remote { principal, request },
                    clients: Default::default(),
                },
                cancellation,
            )
            .map_err(|error| Error::Host(error.to_string()))?;
        self.track_plugin_scope(scope, Some(resources))
    }

    fn track_plugin_scope(
        &self,
        scope: Scope,
        parent: Option<std::sync::Arc<maka_plugins::call::Resources>>,
    ) -> Result<Owned, Error> {
        let evidence = self
            .plugin_calls
            .evidence::<Evidence>(&scope)
            .ok_or(Error::Denied)?;
        let mut ticket = parent
            .map(|resources| resources.reserve())
            .transpose()
            .map_err(|error| Error::Host(error.to_string()))?;
        let call = scope.clone();
        evidence
            .owner
            .spawn_resource("authorized call", move |retiring| async move {
                if let Some(ticket) = &mut ticket {
                    ticket.start();
                }
                tokio::select! {
                    _ = retiring.cancelled() => {},
                    _ = call.cancellation.cancelled() => {},
                }
                let result = call.finish().await.map_err(|error| error.to_string());
                if let Some(ticket) = ticket {
                    ticket.complete(result.clone());
                }
                result
            })
            .map_err(|_| Error::Revoked)?;
        Ok(Owned::new(scope))
    }

    pub(crate) fn plugin_resource_target(&self, scope: &Scope) -> Result<ResourceTarget, Error> {
        if !self.plugin_calls.owns(scope) || scope.cancellation.is_cancelled() {
            return Err(Error::Revoked);
        }
        if let Some(invocation) = scope.identity.agent() {
            return Ok(ResourceTarget::Session(invocation.session_id.clone()));
        }
        match &self
            .plugin_calls
            .evidence::<Evidence>(scope)
            .ok_or(Error::Denied)?
            .boundary
        {
            Boundary::Session { boundary, .. } => {
                Ok(ResourceTarget::Session(boundary.session_id.clone()))
            }
            Boundary::Workspace { workspace, .. } => {
                Ok(ResourceTarget::Workspace(workspace.host_cwd.clone()))
            }
            Boundary::Profile => Err(Error::Denied),
        }
    }

    pub(crate) async fn plugin_resource_boundary(
        &self,
        scope: &Scope,
        capability: Capability,
    ) -> Result<Boundary, Error> {
        if !self.accepting() || !self.plugin_calls.owns(scope) || scope.cancellation.is_cancelled()
        {
            return Err(Error::Revoked);
        }
        let evidence = self
            .plugin_calls
            .evidence::<Evidence>(scope)
            .ok_or(Error::Denied)?;
        let _lease = evidence.owner.admit().map_err(|_| Error::Revoked)?;
        match &evidence.source {
            Source::Background(id) => {
                let record = consent::validate(
                    &self.log,
                    &self.configuration,
                    &evidence.namespace,
                    *id,
                    capability,
                )
                .await
                .map_err(super::authority::consent_error)?;
                Ok(record.boundary)
            }
            Source::Remote { principal, request } => {
                if !request.capabilities.contains(&capability) {
                    return Err(Error::Denied);
                }
                consent::validate_principal(&self.configuration, principal, request)
                    .await
                    .map_err(super::authority::consent_error)?;
                consent::validate_boundary(&self.log, &evidence.boundary)
                    .await
                    .map_err(super::authority::consent_error)?;
                Ok(evidence.boundary.clone())
            }
        }
    }

    pub(crate) async fn plugin_resource_clients(
        &self,
        scope: &Scope,
    ) -> Result<Vec<std::sync::Arc<maka_client_capability::Registration>>, Error> {
        self.plugin_resource_boundary(scope, Capability::ClientCapabilities)
            .await?;
        let evidence = self
            .plugin_calls
            .evidence::<Evidence>(scope)
            .ok_or(Error::Denied)?;
        if let Some(clients) = evidence.clients.get() {
            return Ok(clients.clone());
        }
        let principal = match &evidence.source {
            Source::Remote { principal, .. } => principal.clone(),
            Source::Background(id) => {
                consent::restore(&self.log, &self.configuration, &evidence.namespace, *id)
                    .await
                    .map_err(super::authority::consent_error)?
                    .principal
            }
        };
        let identity = consent::client_identity(&self.configuration, &principal)
            .await
            .map_err(super::authority::consent_error)?;
        let clients = self
            .capabilities
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .published()
            .filter(|registration| {
                let provider = registration.identity();
                registration.available()
                    && (**provider == identity
                        || (provider.trusted()
                            && identity.credential_bound_client_instance_id.as_deref()
                                == Some(identity.client_instance_id.as_str())
                            && provider.capability_owner.as_ref().is_some_and(|owner| {
                                owner.principal_id == identity.principal_id
                                    && owner.client_instance_id == identity.client_instance_id
                            })))
            })
            .cloned()
            .collect();
        Ok(evidence.clients.get_or_init(|| clients).clone())
    }

    pub(crate) async fn plugin_resource_workspace(
        &self,
        scope: &Scope,
        capability: Capability,
    ) -> Result<String, Error> {
        if !self.plugin_calls.owns(scope) || scope.cancellation.is_cancelled() {
            return Err(Error::Revoked);
        }
        if let Some(invocation) = scope.identity.agent() {
            // This branch serves raw network/process operations. File/model/
            // client operations retain their finer-grained journal admission.
            return self.plugin_process_workspace(invocation).await;
        }
        match self.plugin_resource_boundary(scope, capability).await? {
            Boundary::Session { boundary, .. } => Ok(boundary.cwd),
            Boundary::Workspace { workspace, .. } => Ok(workspace.host_cwd),
            Boundary::Profile => Err(Error::Denied),
        }
    }
}
