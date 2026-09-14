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

use crate::message::message;
use crate::tool_message_id;
use crate::{Content, Message, ProjectionError, ToolContent};
use maka_runtime::event::{Fact, RuntimeEvent};
use maka_runtime::model::{ModelPart, ModelStep};
use maka_runtime::tool_call::{ToolCallIdentity, ToolOrigin};
use maka_runtime::tool_output::ToolOutput;

mod limits;
mod state;
use state::CallState;
pub(super) use state::Tools;

impl Tools {
    pub fn accept(
        &mut self,
        event: &RuntimeEvent,
        ts: u64,
        step: &str,
        output: &ModelStep,
        text: Vec<Message>,
    ) -> Result<Vec<Message>, ProjectionError> {
        let anchor = text.last().map(|message| message.id.clone());
        let mut text = text.into_iter();
        let mut rows = Vec::new();
        for (index, part) in output.parts.iter().enumerate() {
            match part {
                ModelPart::Text { .. } => rows.push(
                    text.next()
                        .ok_or(ProjectionError::Invalid("missing accepted text"))?,
                ),
                ModelPart::ToolCall { call } => {
                    let operation = format!("{step}:{}", call.id);
                    let identity = ToolCallIdentity::provider(step.into(), call.id.clone());
                    let metadata = self.insert(
                        &operation,
                        identity,
                        &call.name,
                        &call.input,
                        if call.provider_executed {
                            CallState::ProviderExecuted
                        } else {
                            CallState::AwaitingDispatch
                        },
                        event,
                    )?;
                    rows.push(message(
                        event,
                        ts,
                        tool_message_id(&event.invocation.invocation_id, &operation),
                        Content::ToolCall {
                            tool_name: call.name.clone(),
                            args: call.input.clone(),
                            step_id: anchor.clone(),
                            provider_options: call.provider_options.clone(),
                            provider_executed: Some(call.provider_executed),
                            metadata,
                        },
                    ));
                }
                ModelPart::ToolResult {
                    id,
                    name,
                    output,
                    is_error,
                    ..
                } => {
                    let operation = format!("{step}:{id}");
                    let pending = self
                        .pending
                        .get(&operation)
                        .ok_or(ProjectionError::Invalid("provider result without call"))?;
                    if pending.name != *name || pending.state != CallState::ProviderExecuted {
                        return Err(ProjectionError::Invalid("provider result name mismatch"));
                    }
                    rows.push(self.result(
                        event,
                        ts,
                        &operation,
                        maka_runtime::tool_call::provider_result_id(&event.id, index),
                        *is_error,
                        ToolContent::Json {
                            value: output.clone(),
                        },
                    )?);
                }
            }
        }
        Ok(rows)
    }
    pub fn boundary(
        &mut self,
        event: &RuntimeEvent,
        ts: u64,
        resolved: Option<&ToolOutput>,
    ) -> Result<Vec<Message>, ProjectionError> {
        match &event.fact {
            Fact::ToolDispatched {
                operation_id,
                call,
                name,
                input,
            }
            | Fact::ToolRejected {
                operation_id,
                call,
                name,
                input,
                ..
            } => {
                let rejected = matches!(event.fact, Fact::ToolRejected { .. });
                let mut rows = Vec::new();
                if matches!(call.origin, ToolOrigin::Provider { .. }) {
                    let pending =
                        self.pending
                            .get_mut(operation_id)
                            .ok_or(ProjectionError::Invalid(
                                "provider dispatch without accepted call",
                            ))?;
                    if pending.identity != *call
                        || pending.name != *name
                        || pending.input != *input
                        || pending.state != CallState::AwaitingDispatch
                    {
                        return Err(ProjectionError::Invalid(
                            "dispatch differs from accepted call",
                        ));
                    }
                    pending.state = if rejected {
                        CallState::AwaitingDispatch
                    } else {
                        CallState::Dispatched
                    };
                } else {
                    let metadata = self.insert(
                        operation_id,
                        call.clone(),
                        name,
                        input,
                        if rejected {
                            CallState::AwaitingDispatch
                        } else {
                            CallState::Dispatched
                        },
                        event,
                    )?;
                    rows.push(message(
                        event,
                        ts,
                        tool_message_id(&event.invocation.invocation_id, operation_id),
                        Content::ToolCall {
                            tool_name: name.clone(),
                            args: input.clone(),
                            step_id: None,
                            provider_options: None,
                            provider_executed: None,
                            metadata,
                        },
                    ));
                }
                if let Fact::ToolRejected { reason, .. } = &event.fact {
                    rows.push(self.result(
                        event,
                        ts,
                        operation_id,
                        event.id.clone(),
                        true,
                        ToolContent::Text {
                            text: reason.to_string(),
                        },
                    )?);
                }
                Ok(rows)
            }
            Fact::ToolSettled {
                operation_id,
                outcome,
            } => {
                if !self
                    .pending
                    .get(operation_id)
                    .is_some_and(|pending| pending.state == CallState::Dispatched)
                {
                    return Err(ProjectionError::Invalid("result without dispatch"));
                }
                let (is_error, content) = ToolContent::from_outcome(outcome, resolved)?;
                Ok(vec![self.result(
                    event,
                    ts,
                    operation_id,
                    event.id.clone(),
                    is_error,
                    content,
                )?])
            }
            _ => unreachable!(),
        }
    }
    fn result(
        &mut self,
        event: &RuntimeEvent,
        ts: u64,
        operation: &str,
        id: String,
        is_error: bool,
        content: ToolContent,
    ) -> Result<Message, ProjectionError> {
        let raw_success = matches!(
            event.fact,
            Fact::ToolSettled {
                outcome: maka_runtime::event::ToolOutcome::Succeeded { .. },
                ..
            }
        );
        if !raw_success {
            limits::check(&content, self.limit)?;
        }
        let pending = self
            .pending
            .remove(operation)
            .ok_or(ProjectionError::Invalid("result without call"))?;
        self.bytes -= pending.bytes;
        let row = message(
            event,
            ts,
            id,
            Content::ToolResult {
                tool_use_id: tool_message_id(&event.invocation.invocation_id, operation),
                is_error,
                content,
                metadata: pending.metadata,
            },
        );
        if raw_success {
            limits::check(&row, crate::MAX_TOOL_ROW_BYTES)?;
        }
        Ok(row)
    }
}
