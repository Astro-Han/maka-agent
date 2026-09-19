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
mod environment;
pub(crate) use environment::executor_skills;
pub(crate) use environment::{Admission, Backend, Environment};

pub(super) enum PreparedRun {
    Model(Box<RunInput>),
    Executor(Box<maka_agent::ExecutorInput>),
}
impl PreparedRun {
    pub fn invocation_mut(&mut self) -> &mut Invocation {
        match self {
            Self::Model(input) => &mut input.invocation,
            Self::Executor(input) => &mut input.request.invocation,
        }
    }
    pub fn message(
        &mut self,
        content: maka_runtime::input::MessageInput,
        skills: Option<Box<maka_runtime::skills::SkillInvocationResult>>,
    ) -> Result<()> {
        match self {
            Self::Model(input) => {
                let RunWork::Message {
                    message,
                    skill_invocation,
                    ..
                } = &mut input.work
                else {
                    return Err(internal("Expected a Message Run"));
                };
                *message = content;
                *skill_invocation = skills;
            }
            Self::Executor(input) => {
                if skills.is_some() {
                    return Err(failure(
                        Code::OperationUnavailable,
                        "Executor cannot accept native Skills",
                    ));
                }
                input.request.content = content;
            }
        }
        Ok(())
    }
}
impl From<RunInput> for PreparedRun {
    fn from(input: RunInput) -> Self {
        Self::Model(Box::new(input))
    }
}

#[derive(Clone, Copy)]
pub(super) enum MessageOrigin<'a> {
    Client { root_id: &'a str },
    Successor,
}

impl Executions {
    pub(super) fn native_tools(
        &self,
        cwd: &str,
        profile: Option<maka_protocol::session::SessionToolProfile>,
    ) -> tools::NativeTools {
        tools::NativeTools {
            cwd: cwd.into(),
            profile,
            set: Default::default(),
            log: self.log.clone(),
            writes: self.writes.clone(),
            shells: self.shells.clone(),
            controllers: self.controllers.clone(),
        }
    }
    pub(crate) async fn validate_message_content(
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
    /// Caller holds admission and supplies a committed environment. Workspace
    /// initialization precedes the durable opening; no model/tool effects run here.
    pub(super) async fn prepare_message(
        &self,
        input: TurnStartInput,
        origin: MessageOrigin<'_>,
        request_fingerprint: Option<String>,
        source_messages: Vec<RootSourceMessage>,
        environment: Environment,
    ) -> Result<PreparedRun> {
        let content = &input.content;
        if input.skill_ids.is_some() {
            return Err(internal("Skill selection must be handled by admission"));
        }
        if let MessageOrigin::Client { root_id, .. } = origin {
            self.validate_message_content(&input.session_id, content, root_id)
                .await?;
        }
        let session = environment.session;
        let mut configuration = session.invocation_configuration().await.map_err(internal)?;
        configuration.system_prompt = environment.prompt.clone();
        configuration.tool_composition = Some(environment.composition);
        let invocation = Invocation {
            session_id: input.session_id.clone(),
            turn_id: input.turn_id.clone(),
            run_id: Uuid::new_v4().to_string(),
            invocation_id: Uuid::new_v4().to_string(),
        };
        let prepared_content = if source_messages.is_empty() {
            input.content.into()
        } else {
            maka_runtime::message::aggregate(
                source_messages.iter().map(|source| &source.message.content),
            )
        };
        let model = match environment.backend {
            Backend::Executor(binding) => {
                if input.max_steps.is_some() {
                    return Err(failure(
                        Code::OperationUnavailable,
                        "Executor does not expose a native model-step limit",
                    ));
                }
                return Ok(PreparedRun::Executor(Box::new(maka_agent::ExecutorInput {
                    request: maka_plugins::executor::Request {
                        invocation,
                        conversation_key: input.session_id,
                        content: prepared_content,
                        cwd: configuration.cwd.clone(),
                        instructions: environment.prompt.map(|prompt| prompt.text),
                    },
                    binding,
                    configuration,
                    request_fingerprint,
                    source_messages,
                })));
            }
            Backend::Model(model) => model,
        };
        let provider = provider::resolve(
            &self.configuration,
            &self.oauth,
            &input.session_id,
            &session,
        )
        .await?;
        let tools = model.tools;
        let max_steps = usize::try_from(input.max_steps.unwrap_or(64)).map_err(internal)?;
        Ok(PreparedRun::Model(Box::new(RunInput {
            invocation: invocation.clone(),
            work: RunWork::Message {
                source_messages,
                skill_invocation: Default::default(),
                message: prepared_content,
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
        })))
    }
}
