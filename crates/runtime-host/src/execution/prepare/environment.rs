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
    pub session: SessionConfiguration,
    pub backend: Backend,
    pub prompt: SystemPrompt,
    pub composition: maka_runtime::execution::ToolComposition,
    bindings: Option<PreparedBindings>,
    directory: maka_fs_tools::workspace::directory::PublishedDirectory,
}
pub(crate) enum Backend {
    Model(Box<ModelEnvironment>),
    Executor(maka_plugins::executor::Binding),
}
pub(crate) struct ModelEnvironment {
    behavior: Option<BehaviorBasis>,
    pub tools: maka_tools::ToolCatalog,
    pub skills: Arc<super::super::skills::FrozenSkills>,
}

struct BehaviorBasis {
    source: maka_plugins::contributions::Contribution<maka_plugins::session::SessionBehavior>,
    admission: Option<tokio_util::sync::CancellationToken>,
}

impl Executions {
    pub(crate) async fn prepare_environment(
        &self,
        session_id: &str,
        connection: Option<uuid::Uuid>,
        mode: BindingMode,
        orchestration: Option<maka_runtime::execution::OrchestrationMode>,
    ) -> Result<Environment> {
        let record = self
            .log
            .get_session::<SessionConfiguration>(session_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
        self.prepare_environment_for(record, connection, mode, orchestration)
            .await
    }

    pub(in crate::execution) async fn prepare_environment_for(
        &self,
        record: maka_event_log::sessions::SessionRecord<SessionConfiguration>,
        connection: Option<uuid::Uuid>,
        mode: BindingMode,
        orchestration: Option<maka_runtime::execution::OrchestrationMode>,
    ) -> Result<Environment> {
        let session_id = &record.id;
        let mut session = record.configuration;
        if let Some(mode) = orchestration {
            session.orchestration_mode = mode;
        }
        if record.archived {
            return Err(failure(Code::SessionArchived, "Session is archived"));
        }
        self.prepare_worktree(&session).await?;
        use maka_protocol::session::{CollaborationMode, OrchestrationMode};
        if session.collaboration_mode != CollaborationMode::Agent {
            return Err(failure(
                Code::OperationUnavailable,
                "Plan execution is not installed",
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
        if let crate::session::SessionTarget::Executor { executor_id } = &session.target {
            if orchestration.is_some() {
                return Err(failure(
                    Code::OperationUnavailable,
                    "Executor does not support native Turn orchestration",
                ));
            }
            let binding = self.executor_binding(session_id, executor_id)?;
            let policy = self
                .configuration
                .runtime_policy()
                .await
                .map_err(crate::server::configuration::failure)?;
            let mut prompt = super::super::prompt::resolve(
                policy,
                session.workspace.host_cwd.clone().into(),
                self.paths.global_instructions.clone(),
            )
            .await
            .map_err(internal)?;
            session.append_instructions(&mut prompt).map_err(internal)?;
            let cwd = session.workspace.host_cwd.clone();
            let directory = tokio::task::spawn_blocking(move || {
                maka_fs_tools::workspace::directory::PublishedDirectory::open(std::path::Path::new(
                    &cwd,
                ))
            })
            .await
            .map_err(internal)?
            .map_err(internal)?;
            return Ok(Environment {
                composition: maka_runtime::execution::ToolComposition {
                    clients: bindings.composition(),
                    bound_tools: session.bound_tools.clone(),
                    skills_digest: None,
                },
                bindings: Some(bindings),
                digest: record.configuration_digest,
                session,
                backend: Backend::Executor(binding),
                prompt,
                directory,
            });
        }
        let (behavior, basis) = if session.orchestration_mode != OrchestrationMode::Default {
            let snapshot = self
                .plugin_catalog
                .capture(&maka_plugins::composition::Scope::Profile)
                .typed::<maka_plugins::session::SessionBehavior>();
            let behavior = snapshot.entries.get("agent-graph").ok_or_else(|| {
                failure(
                    Code::OperationUnavailable,
                    "Agent Graph plugin is not active",
                )
            })?;
            let _lease = behavior.admit().map_err(internal)?;
            let preparation = behavior
                .value
                .0
                .prepare(session_id.clone(), session.orchestration_mode)
                .await
                .map_err(internal)?;
            preparation.validate().map_err(internal)?;
            let basis = BehaviorBasis {
                source: behavior.clone(),
                admission: preparation.admission.clone(),
            };
            (preparation, Some(basis))
        } else {
            (maka_plugins::session::Preparation::default(), None)
        };
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
        session.append_instructions(&mut prompt).map_err(internal)?;
        if !behavior.instructions.is_empty() {
            prompt.text.push_str("\n\n");
            prompt.text.push_str(&behavior.instructions);
            prompt.validate().map_err(internal)?;
        }
        let native = self.native_tools(&session.workspace.host_cwd, session.tool_profile);
        let ceiling = session.tool_ceiling(behavior.tool_ceiling);
        let native_ceiling = ceiling.clone();
        let mode = session.permission_mode;
        let (directory, tools, skills, skills_digest) = tokio::task::spawn_blocking(move || {
            let directory = maka_fs_tools::workspace::directory::PublishedDirectory::open(
                std::path::Path::new(&native.cwd),
            )
            .map_err(internal)?;
            let (tools, skills) =
                tools::catalog(native, mode, additional, skills, native_ceiling.as_ref())?;
            let skills_digest = skills.catalog().fingerprint().map_err(internal)?;
            Ok::<_, maka_protocol::OperationError>((directory, tools, skills, skills_digest))
        })
        .await
        .map_err(internal)??;
        let tools = tools
            .with_plugins(
                self.plugin_catalog.clone(),
                maka_plugins::composition::Scope::Session(session_id.clone()),
                ceiling.clone(),
            )
            .map_err(internal)?;
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
                bound_tools: ceiling,
                skills_digest: Some(skills_digest),
            },
            bindings: Some(bindings),
            backend: Backend::Model(Box::new(ModelEnvironment {
                behavior: basis,
                tools,
                skills,
            })),
            digest: record.configuration_digest,
            session,
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
        let Backend::Model(model) = &self.backend else {
            executor_skills(&content, &ids)?;
            return Ok((
                self,
                content,
                super::super::skills::SkillPreparation::Ready {
                    skill_invocation: Default::default(),
                    required_tools: Default::default(),
                },
            ));
        };
        let skills = model.skills.clone();
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
        {
            return Ok(None);
        }
        if executions.retiring() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        self.directory
            .validate(std::path::Path::new(&self.session.workspace.host_cwd))
            .map_err(internal)?;
        match &mut self.backend {
            Backend::Model(model) => {
                if model.behavior.as_ref().is_some_and(|basis| {
                    !basis.source.is_effective()
                        || basis
                            .admission
                            .as_ref()
                            .is_some_and(|gate| gate.is_cancelled())
                }) {
                    return Err(failure(
                        Code::OperationUnavailable,
                        "Prepared Session behavior has retired",
                    ));
                }
                if executions
                    .configuration
                    .skill_preferences()
                    .await
                    .ok()
                    .map(|p| p.revision)
                    != model.skills.preference_revision
                {
                    return Ok(None);
                }
            }
            Backend::Executor(binding) if !binding.is_effective() => {
                return Err(failure(Code::OperationUnavailable, "Executor was retired"));
            }
            Backend::Executor(_) => {}
        }
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

pub(crate) fn executor_skills(
    content: &maka_runtime::input::MessageInput,
    ids: &[String],
) -> Result<()> {
    if !ids.is_empty()
        || content
            .inline_references
            .iter()
            .flatten()
            .any(|reference| reference.kind == maka_runtime::input::InlineReferenceKind::Skill)
    {
        return Err(failure(
            Code::OperationUnavailable,
            "Executor adapters do not accept Maka Skill invocations",
        ));
    }
    Ok(())
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
