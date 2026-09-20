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

//! Durable message provenance shared by admission, execution and recovery.
use crate::{
    execution::BehaviorId,
    input::{DeliveredMessage, MessageInput},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    CurrentTurn,
    NextTurn,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnOrchestration {
    pub mode: BehaviorId,
    pub source: TurnOrchestrationSource,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOrchestrationSource {
    SlashCommand,
    HostApi,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedTurnIntent {
    pub input_selections: crate::input::Selections,
    pub turn_orchestration: Option<TurnOrchestration>,
}
impl SubmittedTurnIntent {
    pub fn validate(&self) -> Result<(), &'static str> {
        crate::input::validate_selections(&self.input_selections)?;
        if self.input_selections.is_empty() && self.turn_orchestration.is_none() {
            return Err("invalid submitted Turn intent");
        }
        Ok(())
    }
}

/// Original admission disposition is retained even when queued messages form a successor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDisposition {
    TurnStarted,
    Steering,
    Followup,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootSourceMessage {
    #[serde(flatten)]
    pub message: DeliveredMessage,
    pub submitted_placement: Placement,
    pub disposition: MessageDisposition,
    pub submitted_intent: Option<SubmittedTurnIntent>,
}
impl RootSourceMessage {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.message.validate()?;
        if let Some(intent) = &self.submitted_intent {
            intent.validate()?;
            if self.submitted_placement != Placement::CurrentTurn {
                return Err("exact Turn intent requires current_turn");
            }
        }
        Ok(())
    }
}

/// The canonical opening must consume exactly its ordered source batch.
pub fn validate_sources(
    content: &MessageInput,
    sources: &[RootSourceMessage],
) -> Result<(), &'static str> {
    crate::input::validate_receipts(&content.preparation)?;
    if sources.is_empty() {
        return Ok(());
    }
    if sources.len() > 64 {
        return Err("too many root source messages");
    }
    let mut ids = HashSet::new();
    let mut bytes = 0usize;
    for source in sources {
        source.validate()?;
        if !ids.insert(&source.message.message_id)
            || (sources.len() != 1
                && (source.disposition == MessageDisposition::TurnStarted
                    || source.submitted_intent.is_some()))
        {
            return Err("conflicting root message sources");
        }
        bytes = bytes.saturating_add(source.message.content.text_bytes());
    }
    if bytes > 64 * 1024
        || content.text_bytes() > 64 * 1024
        || serde_json::to_vec(sources)
            .map_err(|_| "invalid root sources")?
            .len()
            > 1024 * 1024
    {
        return Err("root sources exceed durable capacity");
    }
    if aggregate(sources.iter().map(|s| &s.message.content)) != *content {
        return Err("root content differs from its source messages");
    }
    Ok(())
}

/// Matches the client-source aggregate, including UTF-16 inline reference offsets.
pub fn aggregate<'a>(contents: impl IntoIterator<Item = &'a MessageInput>) -> MessageInput {
    let mut result = MessageInput::from("");
    let mut display = String::new();
    let mut offset = 0u64;
    for (index, content) in contents.into_iter().enumerate() {
        if index > 0 {
            result.text.push_str("\n\n");
            display.push_str("\n\n");
        }
        result.text.push_str(&content.text);
        result
            .preparation
            .extend(content.preparation.iter().cloned());
        let visible = content.display_text.as_deref().unwrap_or(&content.text);
        display.push_str(visible);
        append(&mut result.attachments, content.attachments.as_deref());
        append(&mut result.quotes, content.quotes.as_deref());
        append(
            &mut result.directory_references,
            content.directory_references.as_deref(),
        );
        if let Some(references) = &content.inline_references {
            let target = result.inline_references.get_or_insert_with(Vec::new);
            for reference in references.iter().take(32 - target.len()) {
                let mut reference = reference.clone();
                reference.start = reference.start.saturating_add(offset);
                target.push(reference);
            }
        }
        offset = offset.saturating_add(visible.encode_utf16().count() as u64 + 2);
    }
    if display != result.text {
        result.display_text = Some(display);
    }
    result
}
fn append<T: Clone>(target: &mut Option<Vec<T>>, source: Option<&[T]>) {
    if let Some(source) = source.filter(|items| !items.is_empty()) {
        target
            .get_or_insert_with(Vec::new)
            .extend_from_slice(source);
    }
}
