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

use super::{Executions, Result, SkillPreparation, failure, internal};
use crate::session::SessionConfiguration;
use maka_client_capability::{BindingMode, PreparedBindings};
use maka_event_log::sessions::SessionRecord;
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::input::MessageInput;
use std::{collections::HashSet, sync::Arc};

/// Only the caller knows the queue/steering owner. It must revalidate that owner
/// and original message identity before committing this prepared control basis.
pub(crate) struct PreparedSkillInput {
    digest: String,
    preferences: Option<u64>,
    bindings: Option<PreparedBindings>,
    pub content: MessageInput,
    pub selection: SkillPreparation,
}

impl Executions {
    pub(crate) async fn prepare_skill_input(
        &self,
        session: SessionRecord<SessionConfiguration>,
        mut content: MessageInput,
        connection: uuid::Uuid,
        active_tools: Option<Arc<HashSet<String>>>,
    ) -> Result<PreparedSkillInput> {
        if session.configuration.target.model().is_none() {
            super::super::prepare::executor_skills(&content, &[])?;
            return Ok(PreparedSkillInput {
                digest: session.configuration_digest,
                preferences: None,
                bindings: None,
                content,
                selection: SkillPreparation::Ready {
                    skill_invocation: Default::default(),
                    required_tools: Default::default(),
                },
            });
        }
        let cwd = &session.configuration.workspace.host_cwd;
        let (bindings, skills) = match active_tools {
            Some(tools) => (
                None,
                Arc::new(self.load_skills(cwd, tools.as_ref().clone()).await?),
            ),
            None => {
                let (bindings, mut additional) = self
                    .capabilities
                    .prepare_tools(
                        &session.id,
                        Some(connection),
                        BindingMode::Strict,
                        cwd.clone(),
                        self.interactions.clone(),
                    )
                    .map_err(|error| failure(Code::OperationConflict, &error.to_string()))?;
                additional.push(self.interactions.question_tool());
                let skills = self.load_skills(cwd, Default::default()).await?;
                let native = self.native_tools(cwd, session.configuration.tool_profile);
                let mode = session.configuration.permission_mode;
                let ceiling = session.configuration.bound_tools.clone();
                let (_, skills) = tokio::task::spawn_blocking(move || {
                    super::super::tools::catalog(native, mode, additional, skills, ceiling.as_ref())
                })
                .await
                .map_err(internal)??;
                (Some(bindings), skills)
            }
        };
        let preferences = skills.preference_revision;
        let (content, selection) = tokio::task::spawn_blocking(move || {
            let selection = skills.prepare(&mut content, &[])?;
            Ok::<_, maka_protocol::OperationError>((content, selection))
        })
        .await
        .map_err(internal)??;
        Ok(PreparedSkillInput {
            digest: session.configuration_digest,
            preferences,
            bindings,
            content,
            selection,
        })
    }
}

impl PreparedSkillInput {
    pub(crate) async fn commit(
        mut self,
        executions: &Executions,
        session: &str,
    ) -> Result<Option<Self>> {
        let record = executions
            .log
            .get_session::<SessionConfiguration>(session)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
        if record.archived {
            return Err(failure(Code::SessionArchived, "Session is archived"));
        }
        if record.configuration_digest != self.digest
            || record.configuration.target.model().is_some()
                && executions
                    .configuration
                    .skill_preferences()
                    .await
                    .ok()
                    .map(|p| p.revision)
                    != self.preferences
        {
            return Ok(None);
        }
        if executions.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        if let Some(bindings) = self.bindings.take()
            && !executions
                .capabilities
                .commit_tools(bindings)
                .map_err(|error| failure(Code::OperationConflict, &error.to_string()))?
        {
            return Ok(None);
        }
        Ok(Some(self))
    }
}
