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

use super::{Executions, Result, failure, internal, provider, tools};
use crate::session::SessionConfiguration;
use maka_agent::{RunInput, RunWork};
use maka_protocol::{OperationErrorCode as Code, turn::*};
use maka_runtime::{event::Invocation, message::RootSourceMessage};
use uuid::Uuid;

mod attachments;

pub(super) enum MessageOrigin<'a> {
    Client {
        connection_id: Uuid,
        root_id: &'a str,
    },
    Successor,
}

impl Executions {
    pub(crate) async fn prepare_tools(
        &self,
        session_id: &str,
        session: &SessionConfiguration,
        connection_id: Option<Uuid>,
        binding_mode: maka_client_capability::BindingMode,
    ) -> Result<(
        maka_tools::ToolCatalog,
        std::sync::Arc<super::skills::FrozenSkills>,
    )> {
        let mut additional_tools = self
            .capabilities
            .bind_tools(
                session_id,
                connection_id,
                binding_mode,
                session.workspace.host_cwd.clone(),
                self.interactions.clone(),
            )
            .map_err(|error| {
                failure(
                    if matches!(error, maka_client_capability::BindingError::Draining) {
                        Code::HostDraining
                    } else {
                        Code::OperationConflict
                    },
                    &error.to_string(),
                )
            })?;
        additional_tools.push(self.interactions.question_tool());
        let skills = self
            .load_skills(&session.workspace.host_cwd, Default::default())
            .await?;
        let mode = session.permission_mode;
        let native = self.native_tools(&session.workspace.host_cwd, session.tool_profile);
        tokio::task::spawn_blocking(move || tools::catalog(native, mode, additional_tools, skills))
            .await
            .map_err(internal)?
    }
    pub(super) fn native_tools(
        &self,
        cwd: &str,
        profile: Option<maka_protocol::session::SessionToolProfile>,
    ) -> tools::NativeTools {
        tools::NativeTools {
            cwd: cwd.into(),
            profile,
            log: self.log.clone(),
            writes: self.writes.clone(),
            shells: self.shells.clone(),
            controllers: self.controllers.clone(),
        }
    }
    pub(super) async fn validate_message_content(
        &self,
        session: &str,
        content: &MessageContent,
        root_id: &str,
    ) -> Result<()> {
        if content
            .directory_references
            .as_ref()
            .is_some_and(|references| {
                references
                    .iter()
                    .any(|reference| reference.host_id != root_id)
            })
        {
            return Err(failure(
                Code::OperationUnavailable,
                "Directory references belong to a different Runtime Host",
            ));
        }
        attachments::validate(
            &self.log,
            session,
            content.attachments.as_deref().unwrap_or_default(),
        )
        .await
    }
    /// Caller holds the shared admission gate. Preparation has no model/tool effects.
    pub(super) async fn prepare_message(
        &self,
        input: TurnStartInput,
        origin: MessageOrigin<'_>,
        request_fingerprint: Option<String>,
        source_messages: Vec<RootSourceMessage>,
    ) -> Result<(RunInput, std::sync::Arc<super::skills::FrozenSkills>)> {
        let content = &input.content;
        if input.skill_ids.is_some() {
            return Err(internal("Skill selection must be handled by admission"));
        }
        if input.turn_orchestration.is_some() {
            return Err(failure(
                Code::OperationUnavailable,
                "Turn orchestration execution is not installed",
            ));
        }
        if let MessageOrigin::Client { root_id, .. } = origin {
            self.validate_message_content(&input.session_id, content, root_id)
                .await?;
        }
        let (connection_id, binding_mode) = match origin {
            MessageOrigin::Client { connection_id, .. } => (
                Some(connection_id),
                maka_client_capability::BindingMode::Strict,
            ),
            MessageOrigin::Successor => (None, maka_client_capability::BindingMode::Degrade),
        };
        let session = self
            .log
            .get_session::<SessionConfiguration>(&input.session_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
        if session.archived {
            return Err(failure(
                Code::SessionArchived,
                "Cannot start a Turn in an archived Session",
            ));
        }
        use maka_protocol::session::{CollaborationMode, OrchestrationMode};
        if session.configuration.collaboration_mode != CollaborationMode::Agent
            || session.configuration.orchestration_mode != OrchestrationMode::Default
        {
            return Err(failure(
                Code::OperationUnavailable,
                "Plan and non-default orchestration execution are not installed",
            ));
        }
        let provider = provider::resolve(
            &self.configuration,
            &self.oauth,
            &input.session_id,
            &session.configuration,
        )
        .await?;
        let mut configuration = session
            .configuration
            .invocation_configuration()
            .await
            .map_err(internal)?;
        configuration.system_prompt = Some(
            super::prompt::resolve(
                self.configuration
                    .runtime_policy()
                    .await
                    .map_err(crate::server::configuration::failure)?,
                configuration.cwd.clone().into(),
                self.paths.global_instructions.clone(),
            )
            .await
            .map_err(|error| failure(Code::InternalFailure, &error.to_string()))?,
        );
        let (tools, skills) = self
            .prepare_tools(
                &input.session_id,
                &session.configuration,
                connection_id,
                binding_mode,
            )
            .await?;
        if let Some(prompt) = &mut configuration.system_prompt {
            let fragment = skills
                .catalog()
                .prompt((64 * 1024usize).saturating_sub(prompt.text.len() + 2));
            if !fragment.is_empty() {
                prompt.text.push_str("\n\n");
                prompt.text.push_str(&fragment);
            }
        }
        let max_steps = usize::try_from(input.max_steps.unwrap_or(64)).map_err(internal)?;
        let invocation = Invocation {
            session_id: input.session_id.clone(),
            turn_id: input.turn_id.clone(),
            run_id: Uuid::new_v4().to_string(),
            invocation_id: Uuid::new_v4().to_string(),
        };
        Ok((
            RunInput {
                invocation: invocation.clone(),
                work: RunWork::Message {
                    source_messages,
                    skill_invocation: Default::default(),
                    message: input.content.into(),
                    tools,
                    max_steps,
                },
                request_fingerprint,
                provider: provider.config,
                provider_options: provider.options,
                main_output_limit: provider.main_output_limit,
                supports_vision: provider.supports_vision,
                context: Some(provider.context),
                configuration,
            },
            skills,
        ))
    }
}
