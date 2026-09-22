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

use crate::schedule::Target;
use crate::settings::{Preset as SubagentPreset, Profile as SubagentProfile};
use maka_plugins::execution::{CreateChild, Target as ExecutionTarget};
use maka_runtime::execution::{ModelBinding, SandboxMode};
use serde::Serialize;
use std::{collections::BTreeSet, sync::Arc};

pub(super) struct Definitions {
    pub models: Arc<dyn maka_plugins::llm::Models>,
    pub settings: Arc<crate::settings::Settings>,
    pub commands: Arc<dyn maka_plugins::execution::Commands>,
    pub root: String,
}

mod profile;
use profile::Profile;

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Availability {
    Available,
    Unavailable { reason: Reason },
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    MissingTools,
    ModelUnavailable,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Agent {
    target: Target,
    description: &'static str,
    availability: Availability,
    tools: Option<BTreeSet<String>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Preset {
    target: Target,
    name: String,
    description: String,
    profile: SubagentProfile,
    availability: Availability,
}
#[derive(Serialize)]
struct Executor {
    target: Target,
}
#[derive(Serialize)]
pub(super) struct Listing {
    agents: Vec<Agent>,
    presets: Vec<Preset>,
    /// Executor targets use general agents; native tool ceilings cannot constrain an external backend.
    executors: Vec<Executor>,
}

impl Definitions {
    fn availability(
        capabilities: &maka_plugins::execution::SessionCapabilities,
        profile: Profile,
    ) -> Availability {
        match profile {
            Profile::WebResearch if !capabilities.tools.contains("WebSearch") => {
                Availability::Unavailable {
                    reason: Reason::MissingTools,
                }
            }
            _ => Availability::Available,
        }
    }
    pub(super) async fn list(&self) -> Result<Listing, String> {
        let preferences = self.settings.read().await.map_err(super::error)?;
        let capabilities = self
            .commands
            .capabilities(self.root.clone())
            .await
            .map_err(super::error)?;
        let agents = [
            (
                Profile::General,
                "General-purpose agent using the parent model and permission ceiling.",
            ),
            (
                Profile::LocalRead,
                "Read-only local repository exploration.",
            ),
            (Profile::WebResearch, "Web research using WebSearch only."),
            (
                Profile::Implementation,
                "Implementation in an isolated worktree; requires a clean Git repository. Results include an immutable Git patch.",
            ),
        ]
        .into_iter()
        .map(|(profile, description)| Agent {
            target: Target::Agent { agent_id: profile.id().into() },
            description,
            availability: Self::availability(&capabilities, profile),
            tools: profile.tools(),
        })
        .collect();
        let mut presets = Vec::new();
        for preset in preferences
            .presets
            .into_iter()
            .filter(|preset| preset.enabled)
        {
            let available = self.preset_model(&preset).await.is_ok();
            presets.push(Preset {
                availability: if available {
                    Self::availability(&capabilities, preset.profile.into())
                } else {
                    Availability::Unavailable {
                        reason: Reason::ModelUnavailable,
                    }
                },
                target: Target::Preset {
                    preset_id: preset.id,
                },
                name: preset.name,
                description: preset.description,
                profile: preset.profile,
            });
        }
        let executors = capabilities
            .executors
            .into_iter()
            .map(|id| Executor {
                target: Target::Executor {
                    executor_id: id.as_str().to_owned(),
                },
            })
            .collect();
        Ok(Listing {
            agents,
            presets,
            executors,
        })
    }

    pub(super) async fn resolve(
        &self,
        target: &crate::schedule::Target,
        parent: &maka_plugins::session::View,
        operation_id: String,
        parent_session_id: String,
    ) -> Result<CreateChild, String> {
        let capabilities = self
            .commands
            .capabilities(self.root.clone())
            .await
            .map_err(super::error)?;
        let (profile, name, selected) = match target {
            Target::Agent { agent_id } => {
                let profile = match agent_id.as_str() {
                    "general" => Profile::General,
                    "local-read" => Profile::LocalRead,
                    "web-research" => Profile::WebResearch,
                    "implementation" => Profile::Implementation,
                    _ => {
                        return Err(format!(
                            "Unknown agent {agent_id:?}. Call agent_list and use an available entry's target verbatim."
                        ));
                    }
                };
                (profile, format!("Graph {}", profile.id()), None)
            }
            Target::Preset { preset_id } => {
                let preferences = self.settings.read().await.map_err(super::error)?;
                let preset = preferences
                    .presets
                    .iter()
                    .find(|preset| preset.id == *preset_id && preset.enabled)
                    .ok_or_else(|| format!("Preset {preset_id:?} is missing or disabled. Call agent_list and use an available entry's target verbatim."))?;
                (
                    preset.profile.into(),
                    preset.name.clone(),
                    Some(ExecutionTarget::Model {
                        model: self.preset_model(preset).await?,
                        thinking_level: preset.thinking_level,
                    }),
                )
            }
            Target::Executor { executor_id } => {
                let id = capabilities.executors.iter().find(|id| id.as_str() == executor_id)
                    .ok_or_else(|| format!("Executor {executor_id:?} is unavailable. Call agent_list and use an available entry's target verbatim."))?;
                (
                    Profile::General,
                    format!("Graph {}", id.as_str()),
                    Some(ExecutionTarget::Executor {
                        executor_id: id.clone(),
                        settings: Default::default(),
                    }),
                )
            }
            Target::Operator { .. } => {
                return Err("Expected the original operator definition".into());
            }
        };
        if !matches!(
            Self::availability(&capabilities, profile),
            Availability::Available
        ) {
            return Err(format!(
                "Agent {} requires tools not currently available. Call agent_list and choose an available target",
                profile.id()
            ));
        }
        Ok(CreateChild {
            workspace: matches!(profile, Profile::Implementation)
                .then_some(maka_plugins::execution::ChildWorkspace::IsolatedGit),
            operation_id,
            parent_session_id,
            name,
            target: selected,
            sandbox_mode: Some(match (profile, parent.sandbox_mode) {
                (Profile::LocalRead, _) | (_, SandboxMode::ReadOnly) => SandboxMode::ReadOnly,
                (Profile::General, permission) => permission,
                _ => SandboxMode::WorkspaceWrite,
            }),
            bound_tools: profile.tools(),
            instructions: profile.instructions(),
        })
    }

    async fn preset_model(&self, preset: &SubagentPreset) -> Result<ModelBinding, String> {
        self.models
            .resolve(maka_plugins::llm::Selection::Named {
                connection_slug: preset.connection_slug.clone(),
                model: preset.model.clone(),
            })
            .await
            .map_err(super::error)?
            .map(|choice| choice.model)
            .ok_or_else(|| "Preset model is missing, disabled or retired. Call agent_list and choose an available target, or repair the preset's model in settings.".into())
    }
}
