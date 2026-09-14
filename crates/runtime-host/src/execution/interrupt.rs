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

use super::{Executions, Result, failure};
use crate::server::messages::{capacity, projection};
use maka_event_log::{
    StoreError,
    message_interrupts::{InterruptCommand, InterruptReceipt},
    message_queue::MessageQueue,
};
use maka_protocol::{
    OperationErrorCode as Code,
    message::{InterruptInput, InterruptResult},
    turn::TurnState,
};

impl Executions {
    pub(crate) async fn interrupt(
        &self,
        input: InterruptInput,
        epoch: &str,
    ) -> Result<InterruptResult> {
        if input.origin_host_epoch != epoch {
            return Err(failure(
                Code::OutcomeUnknown,
                "Interrupt outcome is not durable across Host Epochs",
            ));
        }
        let command = InterruptCommand {
            host_epoch: input.origin_host_epoch,
            session_id: input.session_id,
            interrupt_id: input.interrupt_id,
            turn_id: input.turn_id,
            run_id: input.run_id,
        };
        let (fence, completed) = {
            let _admission = self.lock_admission().await;
            if self.shutdown.is_cancelled() {
                return Err(failure(Code::HostDraining, "Host is draining"));
            }
            let active = self
                .active
                .lock()
                .unwrap()
                .get(&command.run_id)
                .filter(|run| {
                    run.invocation.session_id == command.session_id
                        && run.invocation.turn_id == command.turn_id
                })
                .cloned();
            let prior = self
                .log
                .message_interrupt_receipt(&command)
                .await
                .map_err(|e| self.message_storage_error(e))?;
            let receipt = if let Some(prior) = prior {
                prior
            } else {
                let session = self
                    .log
                    .get_session::<serde_json::Value>(&command.session_id)
                    .await
                    .map_err(|e| self.message_storage_error(e))?
                    .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
                if session.archived {
                    return Err(failure(Code::SessionArchived, "Session is archived"));
                }
                loop {
                    let queue = self
                        .log
                        .message_queue(&command.session_id)
                        .await
                        .map_err(|e| self.message_storage_error(e))?;
                    if let Some(active) = &active {
                        capacity::interrupt(
                            projection::project(epoch, &queue),
                            &active.invocation,
                        )?;
                    }
                    match self
                        .log
                        .interrupt_message_queue(
                            &command,
                            queue.revision,
                            active.as_ref().map(|run| &run.invocation),
                        )
                        .await
                    {
                        Err(StoreError::RevisionConflict { .. }) => continue,
                        result => break result.map_err(|e| self.message_storage_error(e))?,
                    }
                }
            };
            let InterruptReceipt::Fenced(fence) = receipt else {
                return Err(failure(
                    Code::OperationConflict,
                    "Interrupt does not match the active root Turn",
                ));
            };
            let completed = if let Some(active) = active {
                // Accepted dispatch is retained by the connection's request owner
                // even after its transport closes. Cancel only after SQL fenced delivery.
                let stopped = self.interactions.stop_run(&active.invocation).await;
                active.cancellation.cancel();
                stopped?;
                Some(active.completed)
            } else {
                None
            };
            (fence, completed)
        };
        // A terminal fact alone does not release native/capability cleanup ownership.
        if let Some(completed) = completed {
            completed.cancelled().await;
        }
        let boundary = self
            .log
            .run_boundary(&command.session_id, &command.run_id)
            .await
            .map_err(|error| failure(Code::OutcomeUnknown, &error.to_string()))?
            .ok_or_else(|| failure(Code::OutcomeUnknown, "Interrupted Run is unavailable"))?;
        let turn = super::snapshot::project(boundary).snapshot;
        if turn.turn_id != command.turn_id
            || turn.run_id != command.run_id
            || !matches!(
                turn.state,
                TurnState::Completed { .. }
                    | TurnState::Failed { .. }
                    | TurnState::Cancelled { .. }
            )
        {
            self.begin_drain();
            return Err(failure(
                Code::OutcomeUnknown,
                "Interrupt cleanup has no proven terminal outcome",
            ));
        }
        let projection = projection::project(
            epoch,
            &MessageQueue {
                revision: fence.revision,
                entries: fence.retracted,
            },
        );
        let retracted = projection::retracted(projection);
        Ok(InterruptResult {
            queue_revision: fence.revision,
            retracted,
            turn,
        })
    }
}
