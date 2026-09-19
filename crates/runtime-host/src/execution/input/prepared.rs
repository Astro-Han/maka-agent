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

use crate::execution::{Executions, Result, failure, internal, skills::SkillPreparation};
use crate::session::SessionConfiguration;
use maka_client_capability::{BindingMode, PreparedBindings};
use maka_event_log::sessions::SessionRecord;
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::input::MessageInput;
use std::{collections::HashSet, sync::Arc};

/// The caller revalidates the queue/steering owner and original message identity.
pub(crate) struct PreparedMessageInput {
    digest: String,
    prepared: Option<maka_plugins::input::Prepared>,
    bindings: Option<PreparedBindings>,
    pub content: MessageInput,
    pub selection: SkillPreparation,
}

impl Executions {
    pub(crate) async fn prepare_message_input(
        &self,
        session: SessionRecord<SessionConfiguration>,
        content: MessageInput,
        connection: uuid::Uuid,
        active_tools: Option<Arc<HashSet<String>>>,
    ) -> Result<PreparedMessageInput> {
        if session.configuration.target.model().is_none() {
            crate::execution::prepare::executor_skills(&content, &[])?;
            return Ok(PreparedMessageInput {
                digest: session.configuration_digest,
                prepared: None,
                bindings: None,
                content,
                selection: SkillPreparation::Ready {
                    skill_invocation: Default::default(),
                    required_tools: Default::default(),
                },
            });
        }
        let cwd = &session.configuration.workspace.host_cwd;
        let (bindings, tools) = match active_tools {
            Some(tools) => (None, tools.iter().cloned().collect()),
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
                let native = self.native_tools(cwd, session.configuration.tool_profile);
                let mode = session.configuration.permission_mode;
                let ceiling = session.configuration.bound_tools.clone();
                let native_ceiling = ceiling.clone();
                let tools = tokio::task::spawn_blocking(move || {
                    crate::execution::tools::catalog(
                        native,
                        mode,
                        additional,
                        native_ceiling.as_ref(),
                    )
                })
                .await
                .map_err(internal)??;
                let tools = tools
                    .with_plugins(
                        self.plugin_catalog.clone(),
                        maka_plugins::composition::Scope::Session(session.id.clone()),
                        ceiling,
                    )
                    .map_err(internal)?
                    .resolve_plugins()
                    .map_err(internal)?
                    .names()
                    .into_iter()
                    .collect();
                (Some(bindings), tools)
            }
        };
        let (prepared, selection) = super::prepare(
            &self.plugin_catalog,
            maka_plugins::input::Request {
                session_id: session.id,
                cwd: cwd.clone(),
                content,
                tools,
                selections: Default::default(),
                cancellation: self.shutdown.child_token(),
            },
        )
        .await?;
        Ok(PreparedMessageInput {
            digest: session.configuration_digest,
            content: prepared.content.clone(),
            prepared: Some(prepared),
            bindings,
            selection,
        })
    }
}
impl PreparedMessageInput {
    pub(crate) async fn commit(
        mut self,
        executions: &Executions,
        session: &str,
    ) -> Result<Option<(Self, Option<maka_plugins::input::Admission>)>> {
        let record = executions
            .log
            .get_session::<SessionConfiguration>(session)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
        if record.archived {
            return Err(failure(Code::SessionArchived, "Session is archived"));
        }
        if record.configuration_digest != self.digest {
            return Ok(None);
        }
        if executions.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let admission = if let Some(prepared) = &self.prepared {
            let Some(admitted) = prepared.admit().map_err(internal)? else {
                return Ok(None);
            };
            Some(admitted)
        } else {
            None
        };
        if let Some(bindings) = self.bindings.take()
            && !executions
                .capabilities
                .commit_tools(bindings)
                .map_err(|error| failure(Code::OperationConflict, &error.to_string()))?
        {
            return Ok(None);
        }
        Ok(Some((self, admission)))
    }
}
