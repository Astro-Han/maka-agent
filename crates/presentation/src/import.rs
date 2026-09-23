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

use crate::{Content, Hidden, Message, ProjectionError, Thinking, ToolContent, ToolMetadata};
use maka_runtime::{event::RuntimeEvent, import};
use serde_json::json;

pub(super) fn project(
    event: &RuntimeEvent,
    record: &import::Record,
    ts: u64,
    max_bytes: usize,
) -> Result<Vec<Message>, ProjectionError> {
    record.validate().map_err(ProjectionError::Invalid)?;
    if serde_json::to_vec(record)
        .map_err(|_| ProjectionError::Invalid("invalid import"))?
        .len()
        > max_bytes
    {
        return Err(ProjectionError::TooLarge);
    }
    let make = |id, content| Message {
        id,
        turn_id: event.invocation.turn_id.clone(),
        ts: record.timestamp.unwrap_or(ts),
        content,
    };
    let content = match &record.content {
        import::Content::User { text } => Content::User {
            text: text.clone(),
            display_text: None,
            attachments: None,
            quotes: None,
            directory_references: None,
            inline_references: None,
        },
        import::Content::Assistant {
            text,
            model,
            thinking,
        } => Content::Assistant {
            text: text.clone(),
            model_id: model.clone().unwrap_or_default(),
            interrupted: false,
            thinking: thinking.as_ref().map(|text| Thinking {
                text: text.clone(),
                provider_options: None,
            }),
            provider_options: None,
        },
        import::Content::Note { text } => Content::SystemNote {
            kind: crate::message::ImportedNoteKind::Imported,
            data: crate::message::ImportedNote { text: text.clone() },
        },
        import::Content::Tool {
            name,
            input,
            output,
            is_error,
        } => {
            let call_id = format!("{}:call", event.id);
            let mut rows = vec![make(
                call_id.clone(),
                Content::ToolCall {
                    tool_name: name.clone(),
                    args: input.clone().unwrap_or_else(|| json!({})),
                    step_id: None,
                    provider_options: None,
                    provider_executed: None,
                    metadata: ToolMetadata::Imported {
                        model_visibility: Hidden::Hidden,
                    },
                },
            )];
            if let Some(output) = output {
                rows.push(make(
                    event.id.clone(),
                    Content::ToolResult {
                        tool_use_id: call_id,
                        is_error: *is_error,
                        content: ToolContent::Json {
                            value: output.clone(),
                        },
                        metadata: ToolMetadata::Imported {
                            model_visibility: Hidden::Hidden,
                        },
                    },
                ));
            }
            return Ok(rows);
        }
    };
    Ok(vec![make(event.id.clone(), content)])
}
