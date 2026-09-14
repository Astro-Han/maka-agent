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

use crate::{Content, Message, ProjectionError, Thinking};
use maka_runtime::event::RuntimeEvent;
use maka_runtime::model::{ModelEvent, ModelPart, ModelStep, TextKind, merge_provider_options};
use serde_json::Value;

mod reasoning;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PartState {
    Open,
    Closed,
}
struct Part {
    provider_id: String,
    message_id: String,
    turn_id: String,
    timestamp: u64,
    kind: TextKind,
    text: String,
    options: Option<Value>,
    state: PartState,
}
enum Observed {
    Text(usize),
    Tool(ModelPart),
}
pub(super) struct Step {
    id: String,
    model: String,
    parts: Vec<Part>,
    order: Vec<Observed>,
    text_bytes: usize,
    limit: usize,
}
impl Step {
    pub fn new(id: String, model: String, limit: usize) -> Self {
        Self {
            id,
            model,
            parts: Vec::new(),
            order: Vec::new(),
            text_bytes: 0,
            limit,
        }
    }
    pub fn check_id(&self, id: &str) -> Result<(), ProjectionError> {
        if self.id == id {
            Ok(())
        } else {
            Err(ProjectionError::Invalid("model step identity changed"))
        }
    }
    pub fn observe(
        &mut self,
        event: &RuntimeEvent,
        timestamp: u64,
        observation: &ModelEvent,
    ) -> Result<(), ProjectionError> {
        match observation {
            ModelEvent::PartStarted {
                id,
                text_kind,
                provider_options,
            } => {
                if self.order.len() >= 128 {
                    return Err(ProjectionError::TooLarge);
                }
                if self.parts.iter().any(|part| part.provider_id == *id) {
                    return Err(ProjectionError::Invalid("duplicate model part"));
                }
                self.order.push(Observed::Text(self.parts.len()));
                self.parts.push(Part {
                    provider_id: id.clone(),
                    message_id: event.id.clone(),
                    turn_id: event.invocation.turn_id.clone(),
                    timestamp,
                    kind: *text_kind,
                    text: String::new(),
                    options: provider_options.clone(),
                    state: PartState::Open,
                });
            }
            ModelEvent::PartDelta {
                id,
                text,
                provider_options,
            } => {
                if text.len() > self.limit.saturating_sub(self.text_bytes) {
                    return Err(ProjectionError::TooLarge);
                }
                let part = self
                    .parts
                    .iter_mut()
                    .find(|part| part.provider_id == *id)
                    .ok_or(ProjectionError::Invalid("unknown model part"))?;
                if part.state != PartState::Open {
                    return Err(ProjectionError::Invalid("delta after part end"));
                }
                self.text_bytes += text.len();
                part.text.push_str(text);
                merge_provider_options(&mut part.options, provider_options.clone());
            }
            ModelEvent::PartFinished {
                id,
                provider_options,
            } => {
                let part = self
                    .parts
                    .iter_mut()
                    .find(|part| part.provider_id == *id)
                    .ok_or(ProjectionError::Invalid("unknown model part"))?;
                if part.state != PartState::Open {
                    return Err(ProjectionError::Invalid("duplicate part end"));
                }
                part.state = PartState::Closed;
                merge_provider_options(&mut part.options, provider_options.clone());
            }
            ModelEvent::ToolCall(call) => self.tool(ModelPart::ToolCall { call: call.clone() })?,
            ModelEvent::ProviderToolResult {
                id,
                name,
                output,
                is_error,
                provider_options,
            } => self.tool(ModelPart::ToolResult {
                id: id.clone(),
                name: name.clone(),
                output: output.clone(),
                is_error: *is_error,
                provider_options: provider_options.clone(),
            })?,
            ModelEvent::ResponseMetadata { .. } | ModelEvent::Finished { .. } => {}
        }
        Ok(())
    }

    fn tool(&mut self, part: ModelPart) -> Result<(), ProjectionError> {
        let bytes = serde_json::to_vec(&part)
            .map_err(|_| ProjectionError::TooLarge)?
            .len();
        if self.order.len() >= 128 || bytes > self.limit.saturating_sub(self.text_bytes) {
            return Err(ProjectionError::TooLarge);
        }
        self.text_bytes += bytes;
        self.order.push(Observed::Tool(part));
        Ok(())
    }

    pub fn complete(self, output: &ModelStep) -> Result<Vec<Message>, ProjectionError> {
        if output.parts.len() != self.order.len() {
            return Err(ProjectionError::Invalid(
                "accepted model parts differ from observations",
            ));
        }
        self.order
            .iter()
            .zip(&output.parts)
            .filter_map(|(observed, accepted)| {
                let part = match observed {
                    Observed::Text(index) => &self.parts[*index],
                    Observed::Tool(part) => {
                        return if part == accepted {
                            None
                        } else {
                            Some(Err(ProjectionError::Invalid(
                                "accepted tool differs from observations",
                            )))
                        };
                    }
                };
                Some((|| {
                    let ModelPart::Text {
                        text_kind,
                        text,
                        provider_options,
                    } = accepted
                    else {
                        return Err(ProjectionError::Invalid("accepted text changed kind"));
                    };
                    if part.state != PartState::Closed
                        || part.kind != *text_kind
                        || part.text != *text
                    {
                        return Err(ProjectionError::Invalid(
                            "accepted model text differs from observations",
                        ));
                    }
                    Ok(part.message(&self.model, provider_options.clone(), false))
                })())
            })
            .collect()
    }

    pub fn partial(&self, failed: bool) -> Vec<Message> {
        self.parts
            .iter()
            .map(|part| {
                let interrupted = failed
                    && !(part.kind == TextKind::Thinking
                        && reasoning::finalized(part.options.as_ref()));
                part.message(&self.model, part.options.clone(), interrupted)
            })
            .collect()
    }
}
impl Part {
    fn message(&self, model: &str, options: Option<Value>, interrupted: bool) -> Message {
        let (text, thinking) = match self.kind {
            TextKind::Text => (self.text.clone(), None),
            TextKind::Thinking => (
                String::new(),
                Some(Thinking {
                    text: self.text.clone(),
                    provider_options: options.clone(),
                }),
            ),
        };
        Message {
            id: self.message_id.clone(),
            turn_id: self.turn_id.clone(),
            ts: self.timestamp,
            content: Content::Assistant {
                text,
                model_id: model.into(),
                interrupted,
                thinking,
                provider_options: options,
            },
        }
    }
}
