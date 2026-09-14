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

use super::{ToolError, ToolFuture};
use crate::event::{EventSink, EventWrite, Fact, Invocation, RuntimeEvent, ToolOutcome};
use crate::tool_call::{ToolCallIdentity, ToolRejection};
use crate::tool_output::{ToolOutput, ToolSuccess};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Execution facts without an executor or a second admission authority.
/// Prepared calls can own one-shot effects instead of pretending to be repeatable.
#[derive(Clone)]
pub struct ToolJournal {
    sink: Arc<dyn EventSink>,
    invocation: Invocation,
}
impl ToolJournal {
    pub fn new(sink: Arc<dyn EventSink>, invocation: Invocation) -> Self {
        Self { sink, invocation }
    }
    pub fn invocation(&self) -> &Invocation {
        &self.invocation
    }
    pub fn reject(
        &self,
        operation_id: String,
        call: ToolCallIdentity,
        name: String,
        input: Value,
        reason: ToolRejection,
    ) -> ToolFuture {
        let sink = self.sink.clone();
        let invocation = self.invocation.clone();
        Box::pin(async move {
            let message = reason.to_string();
            let event = RuntimeEvent::new(
                invocation,
                Fact::ToolRejected {
                    operation_id,
                    call,
                    name,
                    input,
                    reason,
                },
            );
            sink.commit(
                EventWrite::plain(event)
                    .map_err(|error| ToolError::Persistence(error.to_string()))?,
            )
            .await
            .map_err(|error| ToolError::Persistence(error.to_string()))?;
            Err(ToolError::Failed(message))
        })
    }

    /// Journal a one-shot effect whose preparation and policy checks have
    /// already succeeded. Captured admission guards are dropped if T1 fails
    /// or cancellation prevents execution; the closure is never retried.
    pub fn invoke_call_with<T: Into<ToolSuccess> + Send + 'static>(
        &self,
        operation_id: String,
        call: ToolCallIdentity,
        name: String,
        input: Value,
        cancellation: CancellationToken,
        effect: impl FnOnce(CancellationToken) -> ToolFuture<T> + Send + 'static,
    ) -> ToolFuture {
        let sink = self.sink.clone();
        let invocation = self.invocation.clone();
        Box::pin(async move {
            let dispatch = RuntimeEvent::new(
                invocation.clone(),
                Fact::ToolDispatched {
                    operation_id: operation_id.clone(),
                    call,
                    name,
                    input,
                },
            );
            sink.clone()
                .commit(
                    EventWrite::plain(dispatch)
                        .map_err(|error| ToolError::Persistence(error.to_string()))?,
                )
                .await
                .map_err(|error| ToolError::Persistence(error.to_string()))?;

            let result = if cancellation.is_cancelled() {
                drop(effect);
                Err(ToolError::Failed("cancelled before effect".into()))
            } else {
                effect(cancellation).await.map(Into::into)
            };
            let id = uuid::Uuid::new_v4().to_string();
            let recorded_at = std::time::SystemTime::now();
            let (write, result) = match result {
                Ok(value) => {
                    EventWrite::tool_success(id, recorded_at, invocation, operation_id, value)
                        .map(|(write, output)| (write, Ok(output)))
                }
                Err(ToolError::Failed(message)) => EventWrite::plain(RuntimeEvent {
                    id,
                    recorded_at,
                    invocation,
                    fact: Fact::ToolSettled {
                        operation_id,
                        outcome: ToolOutcome::Failed {
                            message: message.clone(),
                        },
                    },
                })
                .map(|write| (write, Err(ToolError::Failed(message)))),
                // Missing durable outcome remains uncertain on reconstruction.
                Err(error) => return Err(error),
            }
            .map_err(|error| ToolError::OutcomeUnknown(error.to_string()))?;
            sink.commit(write)
                .await
                .map_err(|error| ToolError::OutcomeUnknown(error.to_string()))?;
            result.map(ToolOutput::into_json)
        })
    }
}
