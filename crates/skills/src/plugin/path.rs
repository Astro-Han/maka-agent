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

impl Skills {
    pub async fn resolve_path(
        &self,
        input: &ResolvePathInput,
        workspace_files: maka_plugins::filesystem::ReadDirectory,
    ) -> Result<ResolvePathResult, Error> {
        let _call = self.basis.owner.admit().map_err(|_| Error::Retired)?;
        let _view = self.mutations.read().await;
        let (sources, _) = self.governance(&workspace_files).await?;
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
        use crate::SkillScope;
        let files = match location.scope {
            SkillScope::Project => workspace_files,
            SkillScope::Workspace => self
                .data
                .read_only()
                .await
                .map_err(|error| Error::Source(error.to_string()))?,
            SkillScope::User => self
                .inputs
                .open("user-skills")
                .map_err(|error| Error::Source(error.to_string()))?
                .ok_or(Error::Retired)?,
            SkillScope::Custom => {
                return Ok(ResolvePathResult::Rejected {
                    reason: PathRejection::BlockedPath,
                });
            }
        };
        Ok(crate::discovery::artifact::resolve_path(&files, &location, input.target).await)
    }
}
