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

use maka_protocol::{
    session::sources,
    turn::{self, MessageContent, TurnBatchStartInput},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub original: sources::Source,
    pub content: MessageContent,
    pub excluded: Vec<super::resources::Resource>,
}

impl Input {
    pub fn new(original: sources::Source) -> Self {
        Self {
            content: original.content.clone(),
            excluded: vec![],
            original,
        }
    }

    /// Move intact reference tokens by UTF-16 units, never reinterpret a changed token.
    pub fn replace(&mut self, text: String, display: bool) -> Result<(), &'static str> {
        let before = self
            .content
            .display_text
            .as_deref()
            .unwrap_or(&self.content.text);
        let mut content = self.content.clone();
        if display {
            if content.display_text.is_none() {
                return Err("revision-reference");
            }
            content.display_text = Some(text);
        } else {
            content.text = text;
        }
        let after = content.display_text.as_deref().unwrap_or(&content.text);
        if before != after {
            let before: Vec<u16> = before.encode_utf16().collect();
            let after: Vec<u16> = after.encode_utf16().collect();
            let prefix = before
                .iter()
                .zip(&after)
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = before[prefix..]
                .iter()
                .rev()
                .zip(after[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let old_end = before.len() - suffix;
            let new_end = after.len() - suffix;
            for reference in content.inline_references.iter_mut().flatten() {
                let start = reference.start as usize;
                let end = start + reference.value.encode_utf16().count();
                if end <= prefix {
                    continue;
                }
                if start < old_end {
                    return Err("revision-reference");
                }
                reference.start = (start - old_end + new_end) as _;
            }
        }
        self.content = content;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_resources()?;
        // The original resource identities remain immutable; exclusions apply at submission.
        let mut metadata = self.content.clone();
        metadata.text = self.original.content.text.clone();
        metadata.display_text = self.original.content.display_text.clone();
        if let (Some(current), Some(original)) = (
            &mut metadata.inline_references,
            &self.original.content.inline_references,
        ) {
            if current.len() != original.len() {
                return Err("Changed revision references".into());
            }
            for (current, original) in current.iter_mut().zip(original) {
                current.start = original.start;
            }
        }
        if metadata != self.original.content {
            return Err("Changed revision resources".into());
        }
        if self.content.text.len() > 256 * 1024
            || self
                .content
                .display_text
                .as_ref()
                .is_some_and(|text| text.len() > 256 * 1024)
            || self
                .content
                .text
                .chars()
                .chain(
                    self.content
                        .display_text
                        .iter()
                        .flat_map(|text| text.chars()),
                )
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            return Err("Invalid revision text".into());
        }
        Ok(())
    }
}

pub fn batch(
    inputs: &[Input],
    target: &sources::Output,
    turn_id: &str,
) -> Result<TurnBatchStartInput, String> {
    if inputs.len() != target.messages.len() || inputs.is_empty() {
        return Err("revision-changed".into());
    }
    let orchestration = inputs[0].original.turn_orchestration.clone();
    let mut messages = Vec::with_capacity(inputs.len());
    for (input, mapped) in inputs.iter().zip(&target.messages) {
        let original = &input.original;
        if original.message_id != mapped.message_id
            || original.input_selections != mapped.input_selections
            || original.turn_orchestration != mapped.turn_orchestration
            || original.turn_orchestration != orchestration
        {
            return Err("revision-changed".into());
        }
        let mut observed = mapped.content.clone();
        observed.attachments = original.content.attachments.clone();
        if observed != original.content {
            return Err("revision-changed".into());
        }
        let old = original.content.attachments.as_deref().unwrap_or_default();
        let new = mapped.content.attachments.as_deref().unwrap_or_default();
        if old.len() != new.len() {
            return Err("revision-changed".into());
        }
        for (index, (original, mapped)) in old.iter().zip(new).enumerate() {
            let mut metadata = mapped.clone();
            metadata.storage_ref = original.storage_ref.clone();
            if metadata != *original
                || (input.included(&super::resources::Resource::Attachment { index })
                    && !matches!(&mapped.storage_ref,
                turn::StorageRef::SessionFile { session_id, .. } if *session_id == target.session_id))
            {
                return Err("revision-changed".into());
            }
        }
        let mut message = input.message();
        message.content.attachments = mapped
            .content
            .attachments
            .as_ref()
            .map(|attachments| {
                attachments
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        input.included(&super::resources::Resource::Attachment { index: *index })
                    })
                    .map(|(_, attachment)| attachment.clone())
                    .collect()
            })
            .filter(|attachments: &Vec<_>| !attachments.is_empty());
        messages.push(message);
    }
    let request = TurnBatchStartInput {
        session_id: target.session_id.clone(),
        turn_id: turn_id.into(),
        messages,
        turn_orchestration: orchestration,
        max_steps: None,
    };
    turn::decode_turn_batch_start_input(
        &serde_json::to_value(&request).map_err(|_| "revision-invalid")?,
    )
    .map_err(|_| "revision-invalid".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn editing_preserves_utf16_tokens_and_target_owned_resources_without_flattening() {
        let original = sources::decode_output(&json!({"sessionId":"source","turnId":"old","messages":[
            {"messageId":"one","content":{"text":"🦀 @a.rs end","inlineReferences":[
                {"kind":"workspace_file","value":"@a.rs","label":"a.rs","start":3}]}},
            {"messageId":"two","content":{"text":"second"},"inputSelections":{"skills":["review"]}}
        ]})).unwrap();
        let mut inputs: Vec<_> = original.messages.iter().cloned().map(Input::new).collect();
        inputs[0].replace("中文🦀 @a.rs end".into(), false).unwrap();
        assert_eq!(
            inputs[0].content.inline_references.as_ref().unwrap()[0].start,
            5
        );
        assert!(inputs[0].replace("中文🦀 @b.rs end".into(), false).is_err());
        assert_eq!(inputs[0].content.text, "中文🦀 @a.rs end");
        inputs[0]
            .replace("中文🦀 @a.rs changed".into(), false)
            .unwrap();
        inputs[0].validate().unwrap();
        let mut target = original.clone();
        target.session_id = "target".into();
        let request = batch(&inputs, &target, "new").unwrap();
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[1].input_selections["skills"], ["review"]);
        target.messages.swap(0, 1);
        assert!(batch(&inputs, &target, "new").is_err());
        let mut corrupt = inputs[0].clone();
        corrupt.content.inline_references.as_mut().unwrap()[0].value = "@other".into();
        assert!(corrupt.validate().is_err());
    }
}
