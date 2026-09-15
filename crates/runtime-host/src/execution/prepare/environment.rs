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

use super::{Executions, Result, SessionConfiguration, failure, internal, tools};
use maka_client_capability::{BindingMode, PreparedBindings};
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::execution::SystemPrompt;
use std::sync::Arc;

/// Candidate input, never execution authority. Files and schemas are prepared
/// without the admission gate; commit rechecks the mutable control basis.
pub(crate) struct Environment {
    digest: String,
    bindings: Option<PreparedBindings>,
    pub session: SessionConfiguration,
    pub tools: maka_tools::ToolCatalog,
    pub skills: Arc<super::super::skills::FrozenSkills>,
    pub prompt: SystemPrompt,
    pub composition: maka_runtime::execution::ToolComposition,
    directory: maka_fs_tools::workspace::directory::PublishedDirectory,
}

impl Executions {
    pub(crate) async fn prepare_environment(
        &self,
        session_id: &str,
        connection: Option<uuid::Uuid>,
        mode: BindingMode,
    ) -> Result<Environment> {
        let record = self
            .log
            .get_session::<SessionConfiguration>(session_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
        self.prepare_environment_for(record, connection, mode).await
    }

    pub(in crate::execution) async fn prepare_environment_for(
        &self,
        record: maka_event_log::sessions::SessionRecord<SessionConfiguration>,
        connection: Option<uuid::Uuid>,
        mode: BindingMode,
    ) -> Result<Environment> {
        let session_id = &record.id;
        let session = record.configuration;
        if record.archived {
            return Err(failure(Code::SessionArchived, "Session is archived"));
        }
        use maka_protocol::session::{CollaborationMode, OrchestrationMode};
        if session.collaboration_mode != CollaborationMode::Agent
            || session.orchestration_mode != OrchestrationMode::Default
        {
            return Err(failure(
                Code::OperationUnavailable,
                "Plan and non-default orchestration execution are not installed",
            ));
        }
        let (bindings, mut additional) = self
            .capabilities
            .prepare_tools(
                session_id,
                connection,
                mode,
                session.workspace.host_cwd.clone(),
                self.interactions.clone(),
            )
            .map_err(binding_error)?;
        additional.push(self.interactions.question_tool());
        let policy = self
            .configuration
            .runtime_policy()
            .await
            .map_err(crate::server::configuration::failure)?;
        let (skills, prompt) = tokio::join!(
            self.load_skills(&session.workspace.host_cwd, Default::default()),
            super::super::prompt::resolve(
                policy,
                session.workspace.host_cwd.clone().into(),
                self.paths.global_instructions.clone()
            ),
        );
        let skills = skills?;
        let mut prompt = prompt.map_err(internal)?;
        let native = self.native_tools(&session.workspace.host_cwd, session.tool_profile);
        let mode = session.permission_mode;
        let (directory, tools, skills, skills_digest) = tokio::task::spawn_blocking(move || {
            let directory = maka_fs_tools::workspace::directory::PublishedDirectory::open(
                std::path::Path::new(&native.cwd),
            )
            .map_err(internal)?;
            let (tools, skills) = tools::catalog(native, mode, additional, skills)?;
            let skills_digest = skills.catalog().fingerprint().map_err(internal)?;
            Ok::<_, maka_protocol::OperationError>((directory, tools, skills, skills_digest))
        })
        .await
        .map_err(internal)??;
        let fragment = skills
            .catalog()
            .prompt((64 * 1024usize).saturating_sub(prompt.text.len() + 2));
        if !fragment.is_empty() {
            prompt.text.push_str("\n\n");
            prompt.text.push_str(&fragment);
        }
        Ok(Environment {
            composition: maka_runtime::execution::ToolComposition {
                clients: bindings.composition(),
                skills_digest: Some(skills_digest),
            },
            digest: record.configuration_digest,
            bindings: Some(bindings),
            session,
            tools,
            skills,
            prompt,
            directory,
        })
    }
}

impl Environment {
    pub(in crate::execution) async fn expand(
        self,
        mut content: maka_runtime::input::MessageInput,
        ids: Vec<String>,
    ) -> Result<(
        Self,
        maka_runtime::input::MessageInput,
        super::super::skills::SkillPreparation,
    )> {
        let skills = self.skills.clone();
        let (content, selection) = tokio::task::spawn_blocking(move || {
            let selection = skills.prepare(&mut content, &ids)?;
            Ok::<_, maka_protocol::OperationError>((content, selection))
        })
        .await
        .map_err(internal)??;
        Ok((self, content, selection))
    }
    /// Caller has repeated canonical replay/active/queue checks under admission.
    /// None requests a fresh preparation; it has performed no binding effect.
    pub(crate) async fn commit(
        mut self,
        executions: &Executions,
        session_id: &str,
    ) -> Result<Option<Self>> {
        let record = executions
            .log
            .get_session::<SessionConfiguration>(session_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
        if record.archived {
            return Err(failure(Code::SessionArchived, "Session is archived"));
        }
        if record.configuration_digest != self.digest
            || executions
                .configuration
                .runtime_policy()
                .await
                .map_err(crate::server::configuration::failure)?
                .revision
                != self.prompt.policy_revision
            || executions
                .configuration
                .skill_preferences()
                .await
                .ok()
                .map(|p| p.revision)
                != self.skills.preference_revision
        {
            return Ok(None);
        }
        if executions.retiring() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        self.directory
            .validate(std::path::Path::new(&self.session.workspace.host_cwd))
            .map_err(internal)?;
        if !executions
            .capabilities
            .commit_tools(self.bindings.take().expect("candidate binding"))
            .map_err(binding_error)?
        {
            return Ok(None);
        }
        Ok(Some(self))
    }
}

fn binding_error(error: maka_client_capability::BindingError) -> maka_protocol::OperationError {
    failure(
        if matches!(error, maka_client_capability::BindingError::Draining) {
            Code::HostDraining
        } else {
            Code::OperationConflict
        },
        &error.to_string(),
    )
}
