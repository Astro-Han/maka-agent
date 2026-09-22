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

use super::{BoundCommands, Error, SessionConfiguration, storage};
use crate::session::SessionTarget;
use maka_event_log::sessions::{SessionExecutionState, SessionMutation};
use maka_plugins::execution::{Configure, Configured, Target};
use maka_protocol::session::SessionModelTarget;

impl BoundCommands {
    pub(super) async fn configure(&self, input: Configure) -> Result<Configured, Error> {
        input
            .validate()
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let host = self.executions()?;
        let gate = host.interactions.own_admission().await;
        let lease = self.context.admit().map_err(|_| Error::Revoked)?;
        self.authorize(&host, &input.session_id).await?;
        let current = host
            .log
            .get_session::<SessionConfiguration>(&input.session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        if current.revision != input.expected_revision {
            return Ok(Configured::RevisionConflict {
                expected_revision: input.expected_revision,
                actual_revision: current.revision,
            });
        }
        let mut next = current.configuration.clone();
        match input.target {
            Target::Model {
                model,
                thinking_level,
            } => {
                let target = SessionModelTarget::Explicit {
                    connection_id: model.connection_id,
                    connection_slug: model.connection_slug,
                    model: model.model,
                };
                next.target =
                    crate::session::model::resolve(&host.configuration, &target, thinking_level)
                        .await
                        .map_err(|e| Error::Invalid(e.message))?
                        .into();
                next.thinking_level = thinking_level;
                next.connection_locked = true;
            }
            Target::Executor {
                executor_id,
                settings,
            } => {
                if next.bound_tools.is_some() || next.tool_profile.is_some() {
                    return Err(Error::Invalid(
                        "Executor cannot enforce native tool constraints".into(),
                    ));
                }
                host.executor_binding(&input.session_id, &executor_id)
                    .map_err(|e| Error::Invalid(e.message))?;
                next.target = SessionTarget::Executor {
                    executor_id,
                    settings,
                };
                next.thinking_level = None;
                next.connection_locked = false;
            }
        }
        if next != current.configuration
            && (host
                .has_session_work(&input.session_id)
                .await
                .map_err(storage)?
                || current.execution.as_ref().is_some_and(|execution| {
                    matches!(execution.state, SessionExecutionState::Live { .. })
                })
                || !host
                    .log
                    .pending_interactions(&input.session_id)
                    .await
                    .map_err(storage)?
                    .is_empty())
        {
            return Err(Error::Busy);
        }
        if !host.accepting() {
            return Err(Error::Draining);
        }
        if self.submission_stop.is_cancelled() {
            return Err(Error::Revoked);
        }
        // The SQL owner, not the waiting plugin future, owns an admitted mutation.
        let worker = host.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        host.workers.spawn(async move {
            let result = worker
                .log
                .replace_session_metadata(&input.session_id, input.expected_revision, next)
                .await
                .map(|result| match result {
                    SessionMutation::Committed(record) => Configured::Committed {
                        session: Box::new(
                            record.configuration.plugin_view(record.id, record.revision),
                        ),
                    },
                    SessionMutation::RevisionConflict { expected, actual } => {
                        Configured::RevisionConflict {
                            expected_revision: expected,
                            actual_revision: actual,
                        }
                    }
                })
                .map_err(|error| {
                    if matches!(
                        error,
                        maka_event_log::StoreError::CommitUnknown(_)
                            | maka_event_log::StoreError::OperationUnknown
                    ) {
                        worker.begin_drain();
                    }
                    storage(error)
                });
            if let Ok(Configured::Committed { session }) = &result
                && session.revision != input.expected_revision
                && worker
                    .catalog
                    .publish_session(&input.session_id)
                    .await
                    .is_err()
            {
                // Delivery failure cannot undo the committed configuration.
                worker.begin_drain();
            }
            drop(gate);
            drop(lease);
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| Error::OutcomeUnknown("Session configuration owner disappeared".into()))?
    }
}
