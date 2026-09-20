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

use super::*;
use maka_plugins::filesystem::{ReadAuthorization, ReadError};
use maka_runtime::execution::{WorkspaceProjection, WorkspaceTarget};

pub(super) struct ReadGrant {
    pub views: SessionViews,
    pub workspace: WorkspaceProjection,
    pub session: bool,
}
impl ReadGrant {
    pub async fn validate(&self) -> Result<(), Error> {
        let views = &self.views;
        let _lease = views.owner.admit().map_err(|_| Error::Retired)?;
        if views.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let host = views.host().await?;
        let identity = views.owner.identity().map_err(|_| Error::Retired)?;
        let scoped = match identity.scope {
            maka_plugins::composition::Scope::Session(id) => Some(id),
            _ => None,
        };
        let workspace = if self.session || scoped.is_some() {
            let id = views.session_id.as_ref().ok_or(Error::Retired)?;
            if scoped.as_ref().is_some_and(|scope| scope != id) {
                return Err(Error::Retired);
            }
            let record = host
                .log
                .get_session::<SessionConfiguration>(id)
                .await
                .map_err(|error| Error::Provider(error.to_string()))?
                .ok_or(Error::Retired)?;
            if record.archived {
                return Err(Error::Retired);
            }
            record.configuration.workspace
        } else {
            match &self.workspace.target {
                WorkspaceTarget::Project { project_id } => {
                    let project = host
                        .log
                        .get_project(project_id)
                        .await
                        .map_err(|error| Error::Provider(error.to_string()))?
                        .ok_or(Error::Retired)?;
                    crate::server::resolve_project_workspace(project)
                        .await
                        .map_err(|error| Error::Provider(error.message))?
                }
                WorkspaceTarget::HostPath { path } => {
                    if views.access != Access::HostPaths {
                        return Err(Error::Retired);
                    }
                    crate::server::resolve_workspace_path(path.clone())
                        .await
                        .map_err(|error| Error::Provider(error.message))?
                }
            }
        };
        if workspace != self.workspace {
            return Err(Error::Retired);
        }
        Ok(())
    }
}
impl ReadAuthorization for ReadGrant {
    fn check(&self) -> BoxFuture<'_, Result<maka_plugins::call::Ticket, ReadError>> {
        Box::pin(async move {
            self.validate().await.map_err(|error| match error {
                Error::Retired | Error::Cancelled => ReadError::Retired,
                error => ReadError::Invalid(error.to_string()),
            })?;
            self.views
                .resources
                .reserve()
                .map_err(|_| ReadError::Retired)
        })
    }
}
