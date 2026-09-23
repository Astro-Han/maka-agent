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

use super::{BoundCommands, Error, Grant, SessionConfiguration, root, storage};
use maka_plugins::session::import::{Command, ImportState, Receipt};
use sha2::{Digest, Sha256};

impl BoundCommands {
    pub(super) async fn import_session_command(&self, command: Command) -> Result<Receipt, Error> {
        let id = self.root_id(command.operation_id())?;
        let approval = self.root_grant.as_ref().ok_or(Error::Denied)?;
        let host = self.executions()?;
        let lease = self.context.admit().map_err(|_| Error::Revoked)?;
        self.authorize_origin(&host).await?;
        let project = root::observe_workspace(&host, approval).await?;
        let gate = host.interactions.own_admission().await;
        self.authorize_origin(&host).await?;
        root::recheck_project(&host, project).await?;
        if !host.accepting() {
            return Err(Error::Draining);
        }
        if self.submission_stop.is_cancelled() {
            return Err(Error::Revoked);
        }
        let existing = match host
            .log
            .session_import_configuration::<SessionConfiguration>(&id)
            .await
        {
            Ok(configuration) => Some(configuration),
            Err(maka_event_log::StoreError::SessionNotFound) => None,
            Err(error) => return Err(storage(error)),
        };
        if let Some(configuration) = &existing {
            if host
                .log
                .session_creator(&id)
                .await
                .map_err(storage)?
                .as_ref()
                != Some(&self.namespace)
                || configuration.workspace != approval.workspace
                || configuration.workspace_origin != approval.workspace_origin
                || root::rank(configuration.sandbox_mode) > root::rank(approval.sandbox_mode)
                || !configuration
                    .approval_policy
                    .is_subset_of(approval.approval_policy)
            {
                return Err(Error::Denied);
            }
            // A staged import is not an accepted independent Session yet.
            // Recheck all current ceilings before its configuration becomes live.
            if matches!(command, Command::Publish { .. })
                && host
                    .log
                    .session_import_progress(&id)
                    .await
                    .map_err(storage)?
                    .state
                    == ImportState::Collecting
            {
                self.check_root_configuration(&host, configuration, approval)
                    .await?;
            }
        }
        let begin = if let Command::Begin {
            root: request,
            source,
        } = &command
        {
            request
                .validate()
                .map_err(|error| Error::Invalid(error.to_string()))?;
            source
                .validate()
                .map_err(|error| Error::Invalid(error.into()))?;
            if root::rank(request.settings.sandbox_mode) > root::rank(approval.sandbox_mode)
                || !request
                    .settings
                    .approval_policy
                    .is_subset_of(approval.approval_policy)
            {
                return Err(Error::Denied);
            }
            let configuration =
                root::configuration(&host, &id, request, approval, existing.is_none()).await?;
            host.validate_workspace(&configuration)
                .map_err(|error| Error::Invalid(error.message))?;
            let fingerprint = format!(
                "sha256:{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(approval, &command))
                        .map_err(|error| Error::Invalid(error.to_string()))?
                )
            );
            Some((
                maka_event_log::sessions::PluginSession {
                    session_id: id.clone(),
                    creator: self.namespace.clone(),
                    fingerprint,
                    managed: request.managed,
                    authority_session_id: approval
                        .source
                        .as_ref()
                        .map(|source| source.session_id.clone()),
                },
                configuration,
            ))
        } else {
            if existing.is_none() {
                return Err(Error::NotFound);
            }
            None
        };
        let bound = self.clone();
        let approval = approval.clone();
        let worker = host.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        host.workers.spawn(async move {
            let result = async {
                if bound.submission_stop.is_cancelled() {
                    return Err(Error::Revoked);
                }
                let progress = match command {
                    Command::Begin { source, .. } => {
                        let (owner, configuration) =
                            begin.ok_or(Error::Invalid("missing import admission".into()))?;
                        worker
                            .log
                            .begin_session_import(&owner, &source, &configuration, root::now()?)
                            .await
                            .map_err(storage)?
                    }
                    Command::Append {
                        position, records, ..
                    } => worker
                        .log
                        .append_session_import(&id, position, records)
                        .await
                        .map_err(storage)?,
                    Command::Publish { records, .. } => worker
                        .log
                        .publish_session_import(&id, records)
                        .await
                        .map_err(storage)?,
                    Command::Inspect { .. } => worker
                        .log
                        .session_import_progress(&id)
                        .await
                        .map_err(storage)?,
                    Command::Abandon { .. } => worker
                        .log
                        .abandon_session_import(&id)
                        .await
                        .map_err(storage)?,
                };
                if progress.state == ImportState::Published {
                    if let Some(record) = bound.owned_root(&worker, &id, &approval).await? {
                        let current = record.configuration;
                        bound.grants.lock().unwrap().insert(
                            id.clone(),
                            Grant {
                                workspace_origin: current.workspace_origin,
                                boundary_revision: current.boundary_revision,
                                sandbox_mode: current.sandbox_mode,
                                approval_policy: current.approval_policy,
                                cwd: current.workspace.host_cwd,
                            },
                        );
                    }
                    worker.publish_session_change(&id).await;
                } else if progress.state == ImportState::Abandoned {
                    bound.grants.lock().unwrap().remove(&id);
                    worker.request_removal_recovery();
                }
                Ok(Receipt {
                    session_id: id,
                    progress,
                })
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
            .map_err(|_| Error::OutcomeUnknown("Session import owner disappeared".into()))?
    }
}
