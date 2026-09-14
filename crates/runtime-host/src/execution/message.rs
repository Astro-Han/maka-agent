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
use maka_protocol::{
    OperationErrorCode as Code,
    message::{SubmitInput, SubmitResult},
    turn::TurnStartInput,
};
use maka_runtime::{
    input::{DeliveredMessage, MessageInput},
    message::{MessageDisposition, RootSourceMessage, SubmittedTurnIntent},
};
use uuid::Uuid;

mod queued;

impl Executions {
    pub(crate) async fn submit(
        self: &std::sync::Arc<Self>,
        input: SubmitInput,
        connection_id: Uuid,
        root_id: &str,
        epoch: &str,
    ) -> Result<SubmitResult> {
        let _admission = self.lock_admission().await;
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let content: MessageInput = input.content.clone().into();
        let digest = content.content_digest().map_err(internal)?;
        let intent = (input.skill_ids.is_some() || input.turn_orchestration.is_some()).then(|| {
            SubmittedTurnIntent {
                skill_ids: input.skill_ids.clone().unwrap_or_default(),
                turn_orchestration: input.turn_orchestration.clone(),
            }
        });
        if input.origin_host_epoch == epoch
            && let Some(receipt) = self
                .log
                .message_submit_receipt(epoch, &input.session_id, &input.message_id)
                .await
                .map_err(|error| self.message_storage_error(error))?
        {
            if receipt.content_digest != digest
                || receipt.submitted_placement != input.placement
                || receipt.submitted_intent != intent
            {
                return Err(failure(
                    Code::OperationConflict,
                    "Message identity belongs to another input",
                ));
            }
            return queued::result(receipt);
        }
        // Canonical proof precedes epoch/configuration/archive checks. A lost
        // response is not permission to create another root or call a model.
        if let Some(proof) = self
            .log
            .root_message(&input.session_id, &input.message_id)
            .await
            .map_err(internal)?
        {
            let source = proof.source();
            if source.message.submitted_content_digest != digest
                || source.submitted_placement != input.placement
                || source.submitted_intent != intent
            {
                return Err(failure(
                    Code::OperationConflict,
                    "Message identity belongs to another input",
                ));
            }
            return Ok(match source.disposition {
                MessageDisposition::TurnStarted => SubmitResult::TurnStarted {
                    turn_id: proof.opening().event.invocation.turn_id.clone(),
                    skill_invocation: source.skill_invocation.clone(),
                },
                MessageDisposition::Steering => SubmitResult::Steering {
                    skill_invocation: source.skill_invocation.clone(),
                    queue_revision: None,
                },
                MessageDisposition::Followup => SubmitResult::Followup {
                    skill_invocation: source.skill_invocation.clone(),
                    queue_revision: None,
                },
            });
        }
        if input.origin_host_epoch != epoch {
            return Err(failure(
                Code::OutcomeUnknown,
                "Message admission cannot be proven across Host Epochs",
            ));
        }
        if self
            .log
            .message_cancelled(&input.session_id, &input.message_id)
            .await
            .map_err(internal)?
        {
            return Err(failure(
                Code::OperationConflict,
                "Message was durably cancelled",
            ));
        }
        if self
            .log
            .steering_message(&input.session_id, &input.message_id)
            .await
            .map_err(internal)?
            .is_some()
        {
            return Err(failure(
                Code::OutcomeUnknown,
                "Original steering admission receipt is unavailable",
            ));
        }
        let source = RootSourceMessage {
            message: DeliveredMessage {
                message_id: input.message_id.clone(),
                content,
                submitted_content_digest: digest,
            },
            submitted_placement: input.placement,
            disposition: MessageDisposition::TurnStarted,
            skill_invocation: Default::default(),
            submitted_intent: intent,
        };
        let active = self
            .active
            .lock()
            .unwrap()
            .values()
            .find(|run| run.invocation.session_id == input.session_id)
            .map(|run| run.invocation.clone());
        if let Some(invocation) = active {
            return self
                .queue_message(epoch, invocation, source, root_id, connection_id)
                .await;
        }
        if !self
            .log
            .pending_messages(&input.session_id)
            .await
            .map_err(internal)?
            .is_empty()
        {
            return Err(failure(
                Code::SessionBusy,
                "Session has live or pending work",
            ));
        }
        self.log
            .validate_message_identity(&input.session_id, &input.message_id)
            .await
            .map_err(|error| match error {
                maka_event_log::StoreError::InvalidTransition(reason) => {
                    failure(Code::OperationConflict, &reason)
                }
                other => internal(other),
            })?;
        let (mut run, skills) = self
            .prepare_message(
                TurnStartInput {
                    session_id: input.session_id,
                    turn_id: Uuid::new_v4().to_string(),
                    content: input.content,
                    // Original skill intent stays in the source identity.
                    // Preparation below uses this Run's actual frozen tools.
                    skill_ids: None,
                    turn_orchestration: input.turn_orchestration,
                    max_steps: None,
                },
                super::prepare::MessageOrigin::Client {
                    connection_id,
                    root_id,
                },
                None,
                vec![source],
            )
            .await?;
        let skill_invocation = match &mut run.work {
            maka_agent::RunWork::Message {
                message,
                source_messages,
                ..
            } => {
                let mut source = source_messages
                    .pop()
                    .ok_or_else(|| internal("Message Run omitted its source"))?;
                match skills.prepare(
                    &mut source.message.content,
                    source
                        .submitted_intent
                        .as_ref()
                        .map(|intent| intent.skill_ids.as_slice())
                        .unwrap_or_default(),
                )? {
                    super::skills::SkillPreparation::Ready {
                        skill_invocation, ..
                    } => {
                        source.skill_invocation = skill_invocation;
                        *message = source.message.content.clone();
                        let result = source.skill_invocation.clone();
                        source_messages.push(source);
                        result
                    }
                    super::skills::SkillPreparation::Blocked(skill_invocation) => {
                        return Ok(SubmitResult::Blocked { skill_invocation });
                    }
                }
            }
            _ => return Err(internal("Message preparation produced a non-message Run")),
        };
        // Opening and original source identity commit together before any model
        // or tool effect. There is no separately accepted, unstarted idle row.
        let turn = self.launch(run).await.map_err(|error| {
            if error.code == Code::InternalFailure && self.shutdown.is_cancelled() {
                failure(Code::OutcomeUnknown, &error.message)
            } else {
                error
            }
        })?;
        Ok(SubmitResult::TurnStarted {
            turn_id: turn.turn_id,
            skill_invocation,
        })
    }
}
