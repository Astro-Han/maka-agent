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
use maka_protocol::{OperationErrorCode as Code, turn::*};
use sha2::{Digest, Sha256};
use uuid::Uuid;

impl Executions {
    pub(crate) async fn start(
        self: &std::sync::Arc<Self>,
        input: TurnStartInput,
        connection_id: Uuid,
        root_id: &str,
    ) -> Result<TurnStartResult> {
        let fingerprint = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&input).map_err(internal)?)
        );
        self.start_inputs(
            TurnBatchStartInput {
                session_id: input.session_id,
                turn_id: input.turn_id,
                messages: vec![TurnStartMessage {
                    content: input.content,
                    input_selections: input.input_selections,
                }],
                turn_orchestration: input.turn_orchestration,
                max_steps: input.max_steps,
            },
            fingerprint,
            connection_id,
            root_id,
        )
        .await
    }

    pub(crate) async fn start_batch(
        self: &std::sync::Arc<Self>,
        input: TurnBatchStartInput,
        connection_id: Uuid,
        root_id: &str,
    ) -> Result<TurnStartResult> {
        let fingerprint = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&input).map_err(internal)?)
        );
        self.start_inputs(input, fingerprint, connection_id, root_id)
            .await
    }

    async fn start_inputs(
        self: &std::sync::Arc<Self>,
        input: TurnBatchStartInput,
        fingerprint: String,
        connection_id: Uuid,
        root_id: &str,
    ) -> Result<TurnStartResult> {
        self.ordinary_session(&input.session_id).await?;
        let mut prepared = None;
        loop {
            let admission = self.lock_admission().await;
            if self.shutdown.is_cancelled() {
                return Err(failure(Code::HostDraining, "Host is draining"));
            }
            if let Some(record) = self.recorded(&input.session_id, &input.turn_id).await? {
                if record.fingerprint.as_deref() != Some(&fingerprint) {
                    return Err(failure(
                        Code::OperationConflict,
                        "Turn identity belongs to another request",
                    ));
                }
                return Ok(TurnStartResult::Started {
                    turn: record.snapshot,
                    preparation: record.preparation,
                });
            }
            if self
                .active
                .lock()
                .unwrap()
                .values()
                .any(|run| run.invocation.session_id == input.session_id)
            {
                return Err(failure(Code::SessionBusy, "Session has an active Run"));
            }
            if !self
                .log
                .pending_messages(&input.session_id)
                .await
                .map_err(internal)?
                .is_empty()
            {
                return Err(failure(Code::SessionBusy, "Session has pending Messages"));
            }
            let Some(candidate) = prepared.take() else {
                drop(admission);
                prepared = Some(
                    async {
                        let mut environment = self
                            .prepare_environment(
                                &input.session_id,
                                Some(connection_id),
                                maka_client_capability::BindingMode::Strict,
                                input
                                    .turn_orchestration
                                    .as_ref()
                                    .map(|intent| intent.mode.clone()),
                            )
                            .await?;
                        let mut contents = Vec::with_capacity(input.messages.len());
                        let mut blocked = None;
                        for message in &input.messages {
                            let (next, content, selection) = environment
                                .expand(
                                    message.content.clone().into(),
                                    message.input_selections.clone(),
                                )
                                .await?;
                            environment = next;
                            contents.push(content);
                            if let super::input::Outcome::Blocked { message } = selection {
                                blocked = Some(message);
                                break;
                            }
                        }
                        Ok::<_, maka_protocol::OperationError>((environment, contents, blocked))
                    }
                    .await,
                );
                continue;
            };
            let (environment, contents, blocked) = candidate?;
            let Some((environment, _input_admission)) =
                environment.commit(self, &input.session_id).await?
            else {
                continue;
            };
            let preparation = contents
                .iter()
                .flat_map(|content| content.preparation.iter().cloned())
                .collect::<Vec<_>>();
            maka_runtime::input::validate_receipts(&preparation)
                .map_err(|error| failure(Code::OperationUnavailable, error))?;
            // Keep the Started/Blocked receipt below the transport envelope
            // before starting any Run, not after an unencodable acknowledgement.
            if serde_json::to_vec(&preparation).map_err(internal)?.len() > 700 * 1024 {
                return Err(failure(
                    Code::OperationUnavailable,
                    "Turn preparation receipts exceed response capacity",
                ));
            }
            if let Some(message) = blocked {
                return Ok(TurnStartResult::Blocked {
                    message,
                    preparation,
                });
            }
            if contents
                .iter()
                .map(|content| content.inline_references.as_ref().map_or(0, Vec::len))
                .sum::<usize>()
                > 32
            {
                return Err(failure(
                    Code::OperationUnavailable,
                    "Prepared Turn inputs exceed inline reference capacity",
                ));
            }
            let content = maka_runtime::message::aggregate(&contents);
            let mut sources = Vec::with_capacity(contents.len());
            for (message, content) in input.messages.iter().zip(contents) {
                let unprepared_content: maka_runtime::input::MessageInput =
                    message.content.clone().into();
                let source = maka_runtime::message::RootSourceMessage {
                    message: maka_runtime::input::DeliveredMessage {
                        message_id: Uuid::new_v4().to_string(),
                        content: content.clone(),
                        submitted_content_digest: unprepared_content
                            .content_digest()
                            .map_err(internal)?,
                    },
                    unprepared_content,
                    submitted_placement: maka_runtime::message::Placement::CurrentTurn,
                    disposition: maka_runtime::message::MessageDisposition::TurnStarted,
                    submitted_intent: (!message.input_selections.is_empty()
                        || input.turn_orchestration.is_some())
                    .then(|| maka_runtime::message::SubmittedTurnIntent {
                        input_selections: message.input_selections.clone(),
                        turn_orchestration: input.turn_orchestration.clone(),
                    }),
                };
                sources.push(source);
            }
            maka_runtime::message::validate_sources(&content, &sources)
                .map_err(|error| failure(Code::OperationUnavailable, error))?;
            let input = TurnStartInput {
                session_id: input.session_id,
                turn_id: input.turn_id,
                content: content.clone().into(),
                input_selections: Default::default(),
                turn_orchestration: input.turn_orchestration,
                max_steps: input.max_steps,
            };
            let mut run = self
                .prepare_message(
                    input,
                    super::prepare::MessageOrigin::Client { root_id },
                    Some(fingerprint),
                    sources,
                    environment,
                )
                .await?;
            run.message(content)?;
            let turn = self.launch(run).await?;
            return Ok(TurnStartResult::Started { turn, preparation });
        }
    }
}
