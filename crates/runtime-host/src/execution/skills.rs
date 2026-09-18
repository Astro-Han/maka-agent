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

use super::{Executions, Result, failure, internal};
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::{
    input::{InlineReferenceKind, MessageInput},
    message::RootSourceMessage,
    skills::SkillInvocationResult,
};
use maka_skills::{Catalog, HostCapabilities, Preferences, PreparedInvocation, Source};
use std::{
    collections::{BTreeSet, HashSet},
    path::Path,
    sync::Arc,
};

pub(crate) enum SkillPreparation {
    Ready {
        skill_invocation: SkillInvocationResult,
        required_tools: BTreeSet<String>,
    },
    Blocked(SkillInvocationResult),
}

pub(crate) struct FrozenSkills {
    pub preference_revision: Option<u64>,
    pub discovery: maka_skills::DiscoverySnapshot,
    pub preferences: Preferences,
    pub host: HostCapabilities,
}

mod model;
mod prepared;
pub(crate) use prepared::PreparedSkillInput;

/// A successor checks only the batch it will consume. Other queued messages
/// keep their own target and prerequisites for a later Run.
pub(super) fn validate_pending_tools(
    input: &super::prepare::PreparedRun,
    queue: &[maka_event_log::message_admissions::PendingMessageAdmission],
    sources: &[RootSourceMessage],
) -> Result<()> {
    let names: HashSet<_> = match input {
        super::prepare::PreparedRun::Model(input) => {
            let maka_agent::RunWork::Message { tools, .. } = &input.work else {
                return Err(internal("Message successor omitted its tool catalog"));
            };
            tools.names().into_iter().collect()
        }
        super::prepare::PreparedRun::Executor(_) => HashSet::new(),
    };
    let selected: HashSet<_> = sources
        .iter()
        .map(|source| &source.message.message_id)
        .collect();
    if queue
        .iter()
        .filter(|entry| selected.contains(&entry.source.message.message_id))
        .any(|entry| !entry.required_tools.iter().all(|name| names.contains(name)))
    {
        return Err(failure(
            Code::OperationUnavailable,
            "Successor lacks tools required by the queued Skills",
        ));
    }
    Ok(())
}

impl Executions {
    pub(crate) async fn skill_governance(
        &self,
        cwd: &str,
    ) -> Result<(
        maka_skills::SourceCatalog,
        Option<maka_config::skills::SkillPreferences>,
    )> {
        let preferences = self.configuration.skill_preferences().await.ok();
        let root = self.paths.state_root.clone();
        let home = self.paths.skill_home.clone();
        let cwd = std::path::PathBuf::from(cwd);
        let cancellation = self.shutdown.clone();
        let sources = tokio::task::spawn_blocking(move || {
            maka_skills::governance_catalog(&cwd, &root, home.as_deref(), &cancellation)
        })
        .await
        .map_err(internal)?;
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let sources = sources.map_err(|e| failure(Code::PersistenceFailed, &e.to_string()))?;
        Ok((sources, preferences))
    }
    pub(crate) async fn skill_sources(&self) -> Result<maka_skills::SourceCatalog> {
        let root = self.paths.state_root.clone();
        let home = self.paths.skill_home.clone();
        let cancellation = self.shutdown.clone();
        let result = tokio::task::spawn_blocking(move || {
            maka_skills::source_catalog(&root, home.as_deref(), &cancellation)
        })
        .await
        .map_err(internal)?;
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        result.map_err(|e| failure(Code::PersistenceFailed, &e.to_string()))
    }
    /// Query-only capability selection; it never binds or creates a Session.
    pub(crate) async fn preview_skills(
        &self,
        session_id: Option<&str>,
        connection_id: uuid::Uuid,
        cwd: &str,
        mode: maka_protocol::session::PermissionMode,
        profile: Option<maka_protocol::session::SessionToolProfile>,
    ) -> Result<Arc<FrozenSkills>> {
        self.preview_tool_catalog(session_id, connection_id, cwd, mode, profile)
            .await
            .map(|(_, skills)| skills)
    }

    pub(super) async fn preview_tool_catalog(
        &self,
        session_id: Option<&str>,
        connection_id: uuid::Uuid,
        cwd: &str,
        mode: maka_protocol::session::PermissionMode,
        profile: Option<maka_protocol::session::SessionToolProfile>,
    ) -> Result<(maka_tools::ToolCatalog, Arc<FrozenSkills>)> {
        let mut additional = self
            .capabilities
            .preview_tools(
                session_id,
                connection_id,
                cwd.into(),
                self.interactions.clone(),
            )
            .map_err(|e| {
                failure(
                    if matches!(e, maka_client_capability::BindingError::Draining) {
                        Code::HostDraining
                    } else {
                        Code::OperationUnavailable
                    },
                    &e.to_string(),
                )
            })?;
        additional.push(self.interactions.question_tool());
        let skills = self.load_skills(cwd, Default::default()).await?;
        let native = self.native_tools(cwd, profile);
        let ceiling = match session_id {
            Some(id) => {
                self.log
                    .get_session::<crate::session::SessionConfiguration>(id)
                    .await
                    .map_err(internal)?
                    .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?
                    .configuration
                    .bound_tools
            }
            None => None,
        };
        let prepared = tokio::task::spawn_blocking(move || {
            super::tools::catalog(native, mode, additional, skills, ceiling.as_ref())
        })
        .await
        .map_err(internal)??;
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        Ok(prepared)
    }

    /// Caller holds admission ownership while the target's frozen catalog is used.
    pub(crate) fn active_tool_names(
        &self,
        invocation: &maka_runtime::event::Invocation,
    ) -> Option<Arc<HashSet<String>>> {
        self.active
            .lock()
            .unwrap()
            .get(&invocation.run_id)
            .filter(|run| run.invocation == *invocation)
            .map(|run| run.tool_names.clone())
    }

    /// Read-only discovery. Admission revalidates the captured preferences before use.
    pub(crate) async fn load_skills(
        &self,
        cwd: &str,
        tools: HashSet<String>,
    ) -> Result<FrozenSkills> {
        let (preference_revision, preferences) = match self.configuration.skill_preferences().await
        {
            Ok(snapshot) => (
                Some(snapshot.revision),
                Preferences::Available(snapshot.entries),
            ),
            Err(_) => (None, Preferences::Unavailable),
        };
        let sources = Source::standard(
            Path::new(cwd),
            &self.paths.state_root,
            self.paths.skill_home.as_deref(),
        );
        let cancellation = self.shutdown.clone();
        let discovery =
            tokio::task::spawn_blocking(move || maka_skills::scan(&sources, &cancellation))
                .await
                .map_err(internal)?
                .map_err(|error| {
                    failure(
                        if self.shutdown.is_cancelled() {
                            Code::HostDraining
                        } else {
                            Code::OperationUnavailable
                        },
                        &error.to_string(),
                    )
                })?;
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let host = HostCapabilities {
            tools,
            capabilities: Default::default(),
        };
        Ok(FrozenSkills {
            preference_revision,
            discovery,
            preferences,
            host,
        })
    }
}

impl FrozenSkills {
    pub fn catalog(&self) -> Catalog<'_> {
        Catalog {
            discovery: &self.discovery,
            preferences: &self.preferences,
            host: &self.host,
        }
    }

    pub fn prepare(&self, content: &mut MessageInput, ids: &[String]) -> Result<SkillPreparation> {
        match self.catalog().prepare_invocation(&content.text, ids) {
            PreparedInvocation::Passthrough => Ok(SkillPreparation::Ready {
                skill_invocation: Default::default(),
                required_tools: Default::default(),
            }),
            PreparedInvocation::Blocked(result) => Ok(SkillPreparation::Blocked(result)),
            PreparedInvocation::Ready {
                text,
                result,
                required_tools,
            } => {
                let display = content.display_text.clone().unwrap_or_else(|| {
                    if !content.text.trim().is_empty() {
                        content.text.clone()
                    } else {
                        result
                            .loaded
                            .iter()
                            .map(|skill| format!("/skill:{}", skill.id))
                            .collect::<Vec<_>>()
                            .join(" ")
                    }
                });
                let mut references = content.inline_references.take().unwrap_or_default();
                references.retain(|reference| reference.kind != InlineReferenceKind::Skill);
                references.extend(maka_skills::inline_references(&result.receipts, &display));
                references.sort_by(|a, b| {
                    a.start.cmp(&b.start).then_with(|| {
                        b.value
                            .encode_utf16()
                            .count()
                            .cmp(&a.value.encode_utf16().count())
                    })
                });
                let mut end = 0;
                let references = references
                    .into_iter()
                    .filter(|reference| {
                        if reference.start < end {
                            return false;
                        }
                        end = reference
                            .start
                            .saturating_add(reference.value.encode_utf16().count() as u64);
                        true
                    })
                    .take(32)
                    .collect::<Vec<_>>();
                content.text = text;
                content.display_text = Some(display);
                content.inline_references = (!references.is_empty()).then_some(references);
                // Preparation does not silently enlarge the durable admission
                // budget or truncate instructions/user content to force success.
                if content.text_bytes() > 64 * 1024
                    || serde_json::to_vec(content).map_err(internal)?.len() > 64 * 1024
                {
                    return Err(failure(
                        Code::OperationConflict,
                        "Prepared skill content exceeds durable admission limits",
                    ));
                }
                result.validate().map_err(internal)?;
                Ok(SkillPreparation::Ready {
                    skill_invocation: result,
                    required_tools,
                })
            }
        }
    }
}
