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

use crate::session::SessionConfiguration;
use maka_plugins::{
    composition::Scope,
    contributions::Catalog,
    execution::{ChildTarget, CreateChild},
};
use maka_runtime::{
    configuration::policy::{SubagentPreset, SubagentProfile},
    execution::{ModelBinding, PermissionMode},
};
use serde::Serialize;
use std::{collections::BTreeSet, sync::Arc};

pub(super) struct Definitions {
    pub sessions: Arc<dyn super::host::Sessions>,
    pub catalog: Catalog,
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
    fn availability(&self, profile: Profile) -> Availability {
        match profile {
            Profile::WebResearch
                if !self
                    .catalog
                    .snapshot::<maka_tools::plugins::PluginTool>(&Scope::Profile)
                    .entries
                    .contains_key("WebSearch") =>
            {
                Availability::Unavailable {
                    reason: Reason::MissingTools,
                }
            }
            _ => Availability::Available,
        }
    }
    pub(super) async fn list(&self) -> Result<Listing, String> {
        let preferences = self.sessions.preferences().await?;
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
            availability: self.availability(profile),
            tools: profile.tools(),
        })
        .collect();
        let presets = preferences
            .presets
            .into_iter()
            .filter(|preset| preset.enabled)
            .map(|preset| {
                let available = Self::preset_model(&preset, &preferences.models).is_ok();
                Preset {
                    availability: if available {
                        self.availability(preset.profile.into())
                    } else {
                        Availability::Unavailable {
                            reason: Reason::ModelUnavailable,
                        }
                    },
                    preset_id: preset.id,
                    name: preset.name,
                    description: preset.description,
                    profile: preset.profile,
                }
            })
            .collect();
        let executors = self
            .catalog
            .snapshot::<maka_plugins::executor::Executor>(&Scope::Profile)
            .entries
            .into_keys()
            .collect();
        Ok(Listing {
            agents,
            presets,
            executors,
        })
    }

    pub(super) async fn resolve(
        &self,
        target: &maka_graph::schedule::Target,
        parent: &SessionConfiguration,
        operation_id: String,
        parent_session_id: String,
    ) -> Result<CreateChild, String> {
        use maka_graph::schedule::Target;
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
                let preferences = self.sessions.preferences().await?;
                let preset = preferences
                    .presets
                    .iter()
                    .find(|preset| preset.id == *preset_id && preset.enabled)
                    .ok_or("Agent preset is missing or disabled")?;
                (
                    preset.profile.into(),
                    preset.name.clone(),
                    Some(ChildTarget::Model {
                        model: Self::preset_model(preset, &preferences.models)?,
                        thinking_level: preset.thinking_level,
                    }),
                    executor_id,
                )
            }
            Target::Operator { .. } => {
                return Err("Expected the original operator definition".into());
            }
        };
        if !matches!(self.availability(profile), Availability::Available) {
            return Err(format!(
                "Agent {} is unavailable: required tools or workspace isolation are absent",
                profile.id()
            ));
        }
        let selected = if let Some(executor) = executor {
            if !matches!(profile, Profile::General) {
                return Err("External executors cannot enforce native agent tool profiles".into());
            }
            Some(ChildTarget::Executor {
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

    fn preset_model(
        preset: &SubagentPreset,
        catalog: &maka_runtime::configuration::ConnectionCatalogSnapshot,
    ) -> Result<ModelBinding, String> {
        let row = catalog
            .connections
            .iter()
            .find(|row| row.slug == preset.connection_slug)
            .ok_or("Preset model connection is missing")?;
        if !row.enabled
            || !row.enabled_model_ids.contains(&preset.model)
            || maka_config::model_catalog::provider_facts(&row.provider_type)
                .map_err(super::error)?
                .retired
        {
            return Err("Preset model is disabled or retired".into());
        }
        Ok(ModelBinding {
            connection_id: row.connection_id.clone(),
            connection_slug: row.slug.clone(),
            model: preset.model.clone(),
        })
    }
}
