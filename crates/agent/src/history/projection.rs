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

use super::{EventRef, images, operation_id, user};
use crate::RunError;
use maka_model::prompt::{AssistantPart, Message, ToolOutput};
use maka_runtime::context::{ModelPurpose, resolve_model_purpose};
use maka_runtime::event::{Fact, ToolOutcome};
use maka_runtime::input::InvocationInput;
use maka_runtime::model::{ModelPart, TextKind};
use maka_runtime::tool_call::ToolOrigin;
use std::collections::{HashMap, HashSet};

pub(super) fn build<'a>(
    events: impl Iterator<Item = EventRef<'a>> + Clone,
    session: &str,
    images: &mut Vec<images::Target<'a>>,
    vision: bool,
    replay: Option<super::Replay<'a>>,
) -> Result<Vec<Message>, RunError> {
    let cuts = replay.map(|policy| super::replay::Cuts::new(events.clone(), policy));
    let mut messages = Vec::new();
    let mut calls = HashMap::new();
    let mut dispatched = HashSet::new();
    let openings: HashMap<_, _> = events
        .clone()
        .filter_map(EventRef::canonical)
        .filter_map(|stored| match &stored.event.fact {
            Fact::InvocationOpened { input, .. } => {
                Some((&stored.event.invocation.invocation_id, input))
            }
            _ => None,
        })
        .collect();
    let mut purposes = HashMap::new();
    for stored in events.clone().filter_map(EventRef::canonical) {
        if stored.event.invocation.session_id != session {
            continue;
        }
        if let Fact::ModelRequested {
            step_id, purpose, ..
        } = &stored.event.fact
        {
            let opening = openings
                .get(&stored.event.invocation.invocation_id)
                .ok_or_else(|| {
                    RunError::ReconciliationRequired("model request lacks canonical opening".into())
                })?;
            let resolved = resolve_model_purpose(opening, *purpose)
                .map_err(|reason| RunError::ReconciliationRequired(reason.into()))?;
            purposes.insert((&stored.event.invocation.invocation_id, step_id), resolved);
        }
    }
    let compact_invocations: HashSet<_> = events
        .clone()
        .filter_map(EventRef::canonical)
        .filter_map(|stored| {
            matches!(
                &stored.event.fact,
                Fact::InvocationOpened {
                    input: InvocationInput::ContextCompact { .. },
                    ..
                }
            )
            .then_some(&stored.event.invocation.invocation_id)
        })
        .collect();
    for event in events {
        let stored = match event {
            EventRef::Canonical(stored) => stored,
            EventRef::Archived(archived) => {
                if archived.invocation.session_id != session {
                    continue;
                }
                let (id, name) = calls.remove(&archived.operation_id).ok_or_else(|| {
                    RunError::ReconciliationRequired("archived result lacks provider call".into())
                })?;
                if !dispatched.remove(&archived.operation_id) {
                    return Err(RunError::ReconciliationRequired(
                        "archived result lacks dispatch".into(),
                    ));
                }
                let maka_runtime::tool_output::DurableToolProjection::Json { value } =
                    &archived.replacement
                else {
                    return Err(RunError::ReconciliationRequired(
                        "archive replacement is not a JSON page".into(),
                    ));
                };
                let output = if archived.is_error {
                    ToolOutput::ErrorJson(value.clone())
                } else {
                    ToolOutput::Json(value.clone())
                };
                messages.push(Message::tool(id, name, output));
                continue;
            }
        };
        if stored.event.invocation.session_id != session {
            continue;
        }
        if compact_invocations.contains(&stored.event.invocation.invocation_id) {
            continue;
        }
        match &stored.event.fact {
            Fact::InvocationOpened { input, .. } => {
                if input.inherited_claim().is_some() {
                    continue;
                }
                let InvocationInput::Message { content, .. } = input else {
                    return Err(RunError::ReconciliationRequired(
                        "invocation has no model-history input".into(),
                    ));
                };
                messages.push(user::project(
                    content,
                    false,
                    messages.len(),
                    images,
                    vision,
                )?);
            }
            Fact::MessageSteered { message, .. } => {
                message
                    .validate()
                    .map_err(|reason| RunError::ReconciliationRequired(reason.into()))?;
                messages.push(user::project(
                    &message.content,
                    true,
                    messages.len(),
                    images,
                    vision,
                )?);
            }
            Fact::ExecutorCompleted { text } => {
                messages.push(Message::Assistant {
                    content: vec![AssistantPart::Text {
                        text: text.clone(),
                        provider_options: None,
                    }],
                    provider_options: None,
                });
            }
            Fact::ModelCompleted { step_id, output } => {
                match purposes.get(&(&stored.event.invocation.invocation_id, step_id)) {
                    Some(ModelPurpose::Summary) => continue,
                    Some(ModelPurpose::Main) => {}
                    None => {
                        return Err(RunError::ReconciliationRequired(
                            "model completion lacks request purpose".into(),
                        ));
                    }
                }
                let mut content = Vec::new();
                for (index, part) in output.parts.iter().enumerate() {
                    let value = match part {
                        // Citations are preserved in the canonical result/UI, not
                        // fabricated as additional model-authored prompt text.
                        ModelPart::Source { .. } => continue,
                        ModelPart::Text {
                            text_kind,
                            text,
                            provider_options,
                        } => match text_kind {
                            TextKind::Thinking => AssistantPart::Reasoning {
                                text: text.clone(),
                                provider_options: provider_options.clone(),
                            },
                            TextKind::Text => AssistantPart::Text {
                                text: text.clone(),
                                provider_options: provider_options.clone(),
                            },
                        },
                        ModelPart::ToolCall { call } => {
                            if !call.provider_executed {
                                calls.insert(
                                    operation_id(step_id, &call.id),
                                    (&call.id, &call.name),
                                );
                            }
                            AssistantPart::ToolCall {
                                tool_call_id: call.id.clone(),
                                tool_name: call.name.clone(),
                                input: call.input.clone(),
                                provider_executed: Some(call.provider_executed),
                                provider_options: call.provider_options.clone(),
                            }
                        }
                        ModelPart::ToolResult {
                            id,
                            name,
                            output,
                            is_error,
                            provider_options,
                        } => AssistantPart::ToolResult {
                            tool_call_id: id.clone(),
                            tool_name: name.clone(),
                            output: if *is_error {
                                ToolOutput::ErrorJson(output.clone())
                            } else {
                                ToolOutput::Json(output.clone())
                            },
                            provider_options: provider_options.clone(),
                        },
                    };
                    if cuts
                        .as_ref()
                        .map(|cuts| cuts.allows(stored, step_id, index, part))
                        .transpose()?
                        .unwrap_or(true)
                    {
                        content.push(value);
                    }
                }
                if !content.is_empty() {
                    messages.push(Message::Assistant {
                        content,
                        provider_options: None,
                    });
                }
            }
            Fact::ToolDispatched {
                operation_id: operation,
                call,
                name,
                ..
            } => match &call.origin {
                ToolOrigin::Provider { step_id } => {
                    if operation_id(step_id, &call.tool_call_id) != *operation
                        || calls.get(operation) != Some(&(&call.tool_call_id, name))
                        || !dispatched.insert(operation)
                    {
                        return Err(RunError::ReconciliationRequired(
                            "provider tool dispatch identity mismatch".into(),
                        ));
                    }
                }
                ToolOrigin::CodeMode { .. }
                | ToolOrigin::CodeCell { .. }
                | ToolOrigin::HostSdk { .. }
                | ToolOrigin::Standalone => {
                    if calls.contains_key(operation) {
                        return Err(RunError::ReconciliationRequired(
                            "hidden tool operation aliases provider call".into(),
                        ));
                    }
                }
            },
            Fact::ToolRejected {
                operation_id: operation,
                call,
                name,
                reason,
                ..
            } => match &call.origin {
                ToolOrigin::Provider { step_id } => {
                    if operation_id(step_id, &call.tool_call_id) != *operation
                        || calls.get(operation) != Some(&(&call.tool_call_id, name))
                        || dispatched.contains(operation)
                    {
                        return Err(RunError::ReconciliationRequired(
                            "provider tool rejection identity mismatch".into(),
                        ));
                    }
                    let (id, name) = calls.remove(operation).expect("validated accepted call");
                    messages.push(Message::tool(
                        id,
                        name,
                        ToolOutput::ErrorText(reason.to_string()),
                    ));
                }
                ToolOrigin::CodeMode { .. }
                | ToolOrigin::CodeCell { .. }
                | ToolOrigin::HostSdk { .. }
                | ToolOrigin::Standalone => {
                    if calls.contains_key(operation) {
                        return Err(RunError::ReconciliationRequired(
                            "hidden tool rejection aliases provider call".into(),
                        ));
                    }
                }
            },
            Fact::ToolSettled {
                operation_id,
                outcome,
            } => {
                if let Some((id, name)) = calls.remove(operation_id) {
                    if !dispatched.remove(operation_id) {
                        return Err(RunError::ReconciliationRequired(
                            "provider tool result lacks matching dispatch".into(),
                        ));
                    }
                    let output = match outcome {
                        ToolOutcome::Succeeded {
                            model_projection, ..
                        } => {
                            super::output::project(model_projection, messages.len(), images, vision)
                        }
                        ToolOutcome::Failed { message } | ToolOutcome::Unknown { message } => {
                            ToolOutput::ErrorText(message.clone())
                        }
                    };
                    messages.push(Message::tool(id, name, output));
                }
            }
            _ => {}
        }
    }
    if !calls.is_empty() {
        return Err(RunError::ReconciliationRequired(
            "model tool calls have no committed outcome".into(),
        ));
    }
    Ok(messages)
}
