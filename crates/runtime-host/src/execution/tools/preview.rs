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

use super::super::{Executions, Result, failure, internal, prepare::PreparedRun};
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::message::RootSourceMessage;
use std::{collections::HashSet, sync::Arc};

/// A successor checks only the batch it will consume. Other queued messages
/// keep their own target and prerequisites for a later Run.
pub(in crate::execution) fn validate_pending_tools(
    input: &PreparedRun,
    queue: &[maka_event_log::message_admissions::PendingMessageAdmission],
    sources: &[RootSourceMessage],
) -> Result<()> {
    let names: HashSet<_> = match input {
        PreparedRun::Model(input) => {
            let maka_agent::RunWork::Message { tools, .. } = &input.work else {
                return Err(internal("Message successor omitted its tool catalog"));
            };
            tools.names().into_iter().collect()
        }
        PreparedRun::Executor(_) => HashSet::new(),
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
            "Successor lacks tools required by accepted inputs",
        ));
    }
    Ok(())
}

impl Executions {
    pub(crate) async fn preview_tool_catalog(
        &self,
        session_id: Option<&str>,
        connection_id: uuid::Uuid,
        cwd: &str,
        mode: maka_protocol::session::PermissionMode,
        profile: Option<maka_protocol::session::SessionToolProfile>,
    ) -> Result<maka_tools::ToolCatalog> {
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
        let native_ceiling = ceiling.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            super::catalog(native, mode, additional, native_ceiling.as_ref())
        })
        .await
        .map_err(internal)??;
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let scope = session_id
            .map(|id| maka_plugins::composition::Scope::Session(id.into()))
            .unwrap_or(maka_plugins::composition::Scope::Profile);
        prepared
            .with_plugins(self.plugin_catalog.clone(), scope, ceiling)
            .map_err(internal)
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
}
