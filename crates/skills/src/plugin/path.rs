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
use super::{Error, Skills};
use crate::api::{PathRejection, ResolvePathInput, ResolvePathResult};
use maka_runtime::execution::WorkspaceProjection;

impl Skills {
    pub async fn resolve_path(
        &self,
        input: &ResolvePathInput,
        workspace: &WorkspaceProjection,
    ) -> Result<ResolvePathResult, Error> {
        let call = self.basis.owner.admit().map_err(|_| Error::Retired)?;
        let view = self.mutations.clone().read_owned().await;
        let (sources, _) = self.governance(&workspace.host_cwd).await?;
        let discovery = &sources.publication.discovery;
        let location = discovery
            .inventory
            .iter()
            .map(|skill| &skill.location)
            .chain(discovery.rejected.iter().map(|skill| &skill.location))
            .chain(sources.publication.empty.iter())
            .find(|location| location.reference == input.reference)
            .cloned();
        let Some(location) = location else {
            return Ok(ResolvePathResult::Rejected {
                reason: PathRejection::Missing,
            });
        };
        let target = input.target;
        tokio::task::spawn_blocking(move || {
            let (_call, _view) = (call, view);
            crate::discovery::artifact::resolve_path(&location, target)
        })
        .await
        .map_err(Error::from)
    }
}
