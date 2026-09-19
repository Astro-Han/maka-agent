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

use super::{Executions, Result, failure};
use crate::session::{PreparedSession, SessionConfiguration};
use maka_event_log::projects::ProjectRecord;
use maka_protocol::{
    OperationErrorCode as Code,
    session::{SessionCreateInput, SessionCreateTarget, WorkspaceTarget},
};

/// Filesystem/model preparation is not Session admission. The final command
/// rechecks this observation under admission before creating canonical work.
pub(crate) struct Creation {
    pub configuration: SessionConfiguration,
    pub project: Option<ProjectRecord>,
}

impl Executions {
    pub(crate) async fn prepare_session(&self, input: SessionCreateInput) -> Result<Creation> {
        let thinking = input.thinking_level;
        if let SessionCreateTarget::Executor { executor_id } = &input.target {
            self.executor_binding(&input.session_id, executor_id)?;
        }
        let prepared = PreparedSession::new(input)
            .map_err(|error| failure(Code::OperationConflict, &error.to_string()))?;
        let project = match prepared.workspace() {
            WorkspaceTarget::Project { project_id } => Some(
                self.log
                    .get_project(project_id)
                    .await
                    .map_err(super::internal)?
                    .ok_or_else(|| failure(Code::NotFound, "Project does not exist"))?,
            ),
            WorkspaceTarget::HostPath { .. } => None,
        };
        let workspace = match &project {
            Some(project) => crate::server::resolve_project_workspace(project.clone()).await?,
            None => {
                let WorkspaceTarget::HostPath { path } = prepared.workspace() else {
                    unreachable!()
                };
                crate::server::resolve_workspace_path(path.clone()).await?
            }
        };
        let configuration = crate::server::resolve_session_configuration(
            &self.configuration,
            prepared,
            thinking,
            workspace,
        )
        .await?;
        Ok(Creation {
            configuration,
            project,
        })
    }

    pub(crate) async fn validate_creation(&self, creation: &Creation) -> Result<()> {
        if let Some(project) = &creation.project
            && self
                .log
                .get_project(&project.id)
                .await
                .map_err(super::internal)?
                .as_ref()
                != Some(project)
        {
            return Err(failure(
                Code::OperationConflict,
                "Project changed during Session preparation",
            ));
        }
        Ok(())
    }
}
