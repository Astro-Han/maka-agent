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

use super::{
    BoundCommands, ChildSession, CreateChild, Error, Grant, PermissionMode, SessionConfiguration,
    storage,
};
use maka_runtime::execution::{BehaviorId, CollaborationMode};
use sha2::{Digest, Sha256};

mod workspace;

impl BoundCommands {
    pub(super) async fn child(&self, request: CreateChild) -> Result<ChildSession, Error> {
        request
            .validate()
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let host = self.executions()?;
        let gate = host.interactions.own_admission().await;
        self.authorize(&host, &request.parent_session_id).await?;
        let lease = self.context.admit().map_err(|_| Error::Revoked)?;
        if !host.accepting() {
            return Err(Error::Draining);
        }
        let parent = host
            .log
            .get_session::<SessionConfiguration>(&request.parent_session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        let identity = serde_json::to_vec(&(
            "plugin-child-v1",
            self.namespace.package(),
            String::from(self.namespace.scope().clone()),
            &request.operation_id,
        ))
        .map_err(|e| Error::Invalid(e.to_string()))?;
        let id = format!("plugin-{:x}", Sha256::digest(identity));
        let fingerprint = format!(
            "sha256:{:x}",
            Sha256::digest(
                serde_json::to_vec(&request).map_err(|e| Error::Invalid(e.to_string()))?
            )
        );
        // File/status work must not hold the Host-wide admission gate.
        drop(gate);
        let worktree = self
            .plan_child_workspace(&host, &parent.configuration, &request, &id, &fingerprint)
            .await?;
        let gate = host.interactions.own_admission().await;
        self.authorize(&host, &request.parent_session_id).await?;
        if !host.accepting() {
            return Err(Error::Draining);
        }
        let current_parent = host
            .log
            .get_session::<SessionConfiguration>(&request.parent_session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        if current_parent.archived
            || current_parent.configuration_digest != parent.configuration_digest
        {
            return Err(Error::Conflict);
        }
        let grants = self.grants.clone();
        let submission_stop = self.submission_stop.clone();
        let worker = host.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        host.workers.spawn(async move {
            let result = async {
                let existing = worker
                    .log
                    .probe_session_create::<SessionConfiguration>(&id, &fingerprint)
                    .await
                    .map_err(storage)?;
                let current = match existing {
                    Some(record) => record,
                    None => {
                        if submission_stop.is_cancelled() {
                            return Err(Error::Revoked);
                        }
                        let mut child = parent.configuration.clone();
                        workspace::bind(&mut child, request.workspace, worktree)?;
                        if let Some(target) = request.target {
                            match target {
                                maka_plugins::execution::ChildTarget::Model {
                                    model,
                                    thinking_level,
                                } => {
                                    child.target = crate::session::model::resolve(
                                        &worker.configuration,
                                        &maka_protocol::session::SessionModelTarget::Explicit {
                                            connection_id: model.connection_id,
                                            connection_slug: model.connection_slug,
                                            model: model.model,
                                        },
                                        thinking_level,
                                    )
                                    .await
                                    .map_err(|e| Error::Invalid(e.message))?
                                    .into();
                                    child.thinking_level = thinking_level;
                                }
                                maka_plugins::execution::ChildTarget::Executor { executor_id } => {
                                    if child.tool_profile.is_some()
                                        || child.bound_tools.is_some()
                                        || request.bound_tools.is_some()
                                    {
                                        return Err(Error::Invalid(
                                            "Executor cannot enforce native tool constraints"
                                                .into(),
                                        ));
                                    }
                                    worker
                                        .executor_binding(&id, &executor_id)
                                        .map_err(|e| Error::Invalid(e.message))?;
                                    child.target =
                                        crate::session::SessionTarget::Executor { executor_id };
                                    child.thinking_level = None;
                                    child.tool_mode = maka_runtime::execution::ToolMode::Direct;
                                }
                            }
                        }
                        if let Some(mode) = request.permission_mode {
                            if permission_rank(mode) > permission_rank(child.permission_mode) {
                                return Err(Error::Denied);
                            }
                            child.permission_mode = mode;
                        }
                        if child.target.model().is_none()
                            && (request.bound_tools.is_some() || child.bound_tools.is_some())
                        {
                            return Err(Error::Invalid(
                                "Executor cannot enforce native tool constraints".into(),
                            ));
                        }
                        child.bound_tools = child.tool_ceiling(request.bound_tools);
                        if let Some(instructions) = request.instructions {
                            let combined = child.instructions.get_or_insert_default();
                            if !combined.is_empty() {
                                combined.push_str("\n\n");
                            }
                            combined.push_str(&instructions);
                            if combined.len() > 16 * 1024 {
                                return Err(Error::Invalid(
                                    "inherited child instructions exceed 16 KiB".into(),
                                ));
                            }
                        }
                        child.name = request.name;
                        child.labels.clear();
                        child.is_flagged = false;
                        child.title_is_manual = true;
                        child.boundary_revision = 0;
                        child.collaboration_mode = CollaborationMode::Agent;
                        child.orchestration_mode = BehaviorId::default();
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_err(|e| Error::Host(e.to_string()))?
                            .as_millis()
                            .try_into()
                            .map_err(|_| Error::Host("system clock overflow".into()))?;
                        worker
                            .log
                            .create_session(&id, &fingerprint, &child, now)
                            .await
                            .map_err(storage)?
                    }
                };
                // Replaying creation does not overwrite subsequently edited data
                // or restore authority wider than the currently granted parent.
                if current.archived
                    || !workspace::matches_parent(
                        &current.configuration,
                        &parent.configuration,
                        request.workspace,
                    )
                    || permission_rank(current.configuration.permission_mode)
                        > permission_rank(parent.configuration.permission_mode)
                    || parent
                        .configuration
                        .bound_tools
                        .as_ref()
                        .is_some_and(|parent| {
                            current
                                .configuration
                                .bound_tools
                                .as_ref()
                                .is_none_or(|child| !child.is_subset(parent))
                        })
                {
                    return Err(Error::Denied);
                }
                grants.lock().unwrap().insert(
                    id.clone(),
                    Grant {
                        boundary_revision: current.configuration.boundary_revision,
                        permission_mode: current.configuration.permission_mode,
                        cwd: current.configuration.workspace.host_cwd,
                    },
                );
                Ok(ChildSession { session_id: id })
            }
            .await;
            if matches!(result, Err(Error::OutcomeUnknown(_))) {
                worker.begin_drain();
            }
            drop(gate);
            drop(lease);
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| Error::OutcomeUnknown("child Session owner disappeared".into()))?
    }
}

fn permission_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Explore => 0,
        PermissionMode::Ask => 1,
        PermissionMode::Bypass => 2,
    }
}
