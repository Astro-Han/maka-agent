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

use crate::settings::{Preset as SubagentPreset, Profile as SubagentProfile};
use maka_plugins::execution::{CreateChild, Target as ExecutionTarget};
use maka_runtime::execution::{ModelBinding, PermissionMode};
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
    agent_id: &'static str,
    description: &'static str,
    availability: Availability,
    tools: Option<BTreeSet<String>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Preset {
    preset_id: String,
    name: String,
    description: String,
    profile: SubagentProfile,
    availability: Availability,
}
#[derive(Serialize)]
pub(super) struct Listing {
    agents: Vec<Agent>,
    presets: Vec<Preset>,
    /// Executor targets use general agents; native tool ceilings cannot constrain an external backend.
    executors: Vec<String>,
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
            agent_id: profile.id(),
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
                preset_id: preset.id,
                name: preset.name,
                description: preset.description,
                profile: preset.profile,
            });
        }
        let executors = capabilities
            .executors
            .into_iter()
            .map(|id| id.as_str().to_owned())
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
        use crate::schedule::Target;
        let (profile, name, selected, executor) = match target {
            Target::Agent {
                agent_id,
                executor_id,
            } => {
                let profile = match agent_id.as_str() {
                    "general" => Profile::General,
                    "local-read" => Profile::LocalRead,
                    "web-research" => Profile::WebResearch,
                    "implementation" => Profile::Implementation,
                    _ => return Err("Agent definition does not exist".into()),
                };
                (
                    profile,
                    format!("Graph {}", profile.id()),
                    None,
                    executor_id,
                )
            }
            Target::Preset {
                preset_id,
                executor_id,
            } => {
                if executor_id.is_some() {
                    return Err("A model preset cannot also select an executor".into());
                }
                let preferences = self.settings.read().await.map_err(super::error)?;
                let preset = preferences
                    .presets
                    .iter()
                    .find(|preset| preset.id == *preset_id && preset.enabled)
                    .ok_or("Agent preset is missing or disabled")?;
                (
                    preset.profile.into(),
                    preset.name.clone(),
                    Some(ExecutionTarget::Model {
                        model: self.preset_model(preset).await?,
                        thinking_level: preset.thinking_level,
                    }),
                    executor_id,
                )
            }
            Target::Operator { .. } => {
                return Err("Expected the original operator definition".into());
            }
        };
        let capabilities = self
            .commands
            .capabilities(self.root.clone())
            .await
            .map_err(super::error)?;
        if !matches!(
            Self::availability(&capabilities, profile),
            Availability::Available
        ) {
            return Err(format!(
                "Agent {} is unavailable: required tools or workspace isolation are absent",
                profile.id()
            ));
        }
        let selected = if let Some(executor) = executor {
            if !matches!(profile, Profile::General) {
                return Err("External executors cannot enforce native agent tool profiles".into());
            }
            Some(ExecutionTarget::Executor {
                executor_id: executor.clone().try_into().map_err(super::error)?,
            })
        } else {
            selected
        };
        Ok(CreateChild {
            workspace: matches!(profile, Profile::Implementation)
                .then_some(maka_plugins::execution::ChildWorkspace::IsolatedGit),
            operation_id,
            parent_session_id,
            name,
            target: selected,
            permission_mode: Some(match (profile, parent.permission_mode) {
                (Profile::LocalRead, _) | (_, PermissionMode::Explore) => PermissionMode::Explore,
                (Profile::General, permission) => permission,
                _ => PermissionMode::Ask,
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
            .ok_or_else(|| "Preset model is missing, disabled or retired".into())
    }
}
