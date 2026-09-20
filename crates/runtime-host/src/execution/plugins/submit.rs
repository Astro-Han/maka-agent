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

use super::protocol;
use super::{BoundCommands, Error, Receipt, Submit, storage};
use maka_event_log::message_admissions::PendingMessageAdmission;
use maka_runtime::{
    event::Invocation,
    input::DeliveredMessage,
    message::{
        MessageDisposition, Placement, RootSourceMessage, SubmittedTurnIntent, TurnOrchestration,
        TurnOrchestrationSource,
    },
};
use uuid::Uuid;

impl BoundCommands {
    pub(super) async fn submit_message(&self, request: Submit) -> Result<Receipt, Error> {
        let digest = request
            .digest()
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let host = self.executions()?;
        let mut prepared = None;
        loop {
            let gate = host.interactions.own_admission().await;
            let lease = self.context.admit().map_err(|_| Error::Revoked)?;
            self.authorize(&host, &request.session_id).await?;
            if let Some(receipt) = host
                .log
                .plugin_execution_receipt(&self.namespace, &request.operation_id)
                .await
                .map_err(storage)?
            {
                return if receipt.content_digest == digest {
                    Ok(receipt)
                } else {
                    Err(Error::Conflict)
                };
            }
            if !host.accepting() {
                return Err(Error::Draining);
            }
            if self.submission_stop.is_cancelled() {
                return Err(Error::Revoked);
            }
            if host
                .has_session_work(&request.session_id)
                .await
                .map_err(storage)?
            {
                return Err(Error::Busy);
            }
            let Some(candidate) = prepared.take() else {
                drop(gate);
                let connection = self
                    .call
                    .as_ref()
                    .and_then(|call| host.plugin_initiating_connection(call));
                prepared = Some(
                    async {
                        host.prepare_environment(
                            &request.session_id,
                            connection,
                            maka_client_capability::BindingMode::Strict,
                            request.orchestration_mode.clone(),
                        )
                        .await?
                        .expand(request.content.clone(), Default::default())
                        .await
                    }
                    .await,
                );
                continue;
            };
            let (environment, content, selection) = candidate.map_err(protocol)?;
            let required_tools = match selection {
                crate::execution::input::Outcome::Ready { required_tools } => required_tools,
                crate::execution::input::Outcome::Blocked { message } => {
                    return Err(Error::Invalid(message));
                }
            };
            host.validate_message_content(
                &request.session_id,
                &content.clone().into(),
                &self.root_id,
            )
            .await
            .map_err(protocol)?;
            let pending = PendingMessageAdmission {
                invocation: Invocation {
                    session_id: request.session_id.clone(),
                    turn_id: Uuid::new_v4().to_string(),
                    run_id: Uuid::new_v4().to_string(),
                    invocation_id: Uuid::new_v4().to_string(),
                },
                steering_invocation: None,
                source: RootSourceMessage {
                    message: DeliveredMessage {
                        message_id: Uuid::new_v4().to_string(),
                        submitted_content_digest: request
                            .content
                            .content_digest()
                            .map_err(|error| Error::Invalid(error.to_string()))?,
                        content,
                    },
                    submitted_placement: if request.orchestration_mode.is_some() {
                        Placement::CurrentTurn
                    } else {
                        Placement::NextTurn
                    },
                    disposition: MessageDisposition::TurnStarted,
                    submitted_intent: request.orchestration_mode.clone().map(|mode| {
                        SubmittedTurnIntent {
                            input_selections: Default::default(),
                            turn_orchestration: Some(TurnOrchestration {
                                mode,
                                source: TurnOrchestrationSource::HostApi,
                            }),
                        }
                    }),
                },
                required_tools,
                admitted_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| Error::Host(e.to_string()))?
                    .as_millis()
                    .try_into()
                    .map_err(|_| Error::Host("clock overflow".into()))?,
            };
            let mut observation = host
                .log
                .session_projection::<super::SessionConfiguration>(&request.session_id)
                .await
                .map_err(storage)?
                .ok_or(Error::NotFound)?;
            observation.message_queue.entries.push(pending.clone());
            crate::server::messages::capacity::validate(&"x".repeat(128), observation)
                .map_err(protocol)?;
            if self.submission_stop.is_cancelled() {
                return Err(Error::Revoked);
            }
            let Some((_environment, input_lease)) = environment
                .commit(&host, &request.session_id)
                .await
                .map_err(protocol)?
            else {
                continue;
            };
            let namespace = self.namespace.clone();
            let (send, receive) = tokio::sync::oneshot::channel();
            let worker = host.clone();
            // Prepared content and its receipt transfer together to Host ownership.
            host.workers.spawn(async move {
                let result = worker
                    .log
                    .admit_plugin_execution(&namespace, request, pending)
                    .await
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
                drop(input_lease);
                drop(gate);
                drop(lease);
                let receipt = result.as_ref().ok().cloned();
                let _ = send.send(result);
                let mut gate = Some(worker.lock_admission().await);
                if let Some(receipt) = receipt
                    && let Err(error) = worker
                        .dispatch_pending(&receipt.invocation.session_id, &mut gate)
                        .await
                {
                    eprintln!("plugin execution dispatch failed: {}", error.message);
                }
            });
            return receive
                .await
                .map_err(|_| Error::OutcomeUnknown("Host command owner disappeared".into()))?;
        }
    }
}
