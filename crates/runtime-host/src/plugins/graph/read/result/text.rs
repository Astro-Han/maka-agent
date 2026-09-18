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

use maka_plugins::remote::Error;
use maka_runtime::{
    event::{Fact, InvocationOutcome, ToolOutcome},
    model::{ModelPart, TextKind},
    tool_output::{DurableToolProjection, ProjectionPart},
};
use serde::Serialize;

const PAGE_BYTES: usize = 4096;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TextPage {
    pub text: String,
    pub offset: usize,
    pub total_bytes: usize,
    pub next_offset: Option<usize>,
}

/// Count all source bytes, retaining only the requested contiguous UTF-8 window.
pub(super) struct Window {
    offset: usize,
    total: usize,
    text: String,
    full: bool,
    invalid: bool,
}
impl Window {
    pub(super) fn new(offset: usize) -> Self {
        Self {
            offset,
            total: 0,
            text: String::new(),
            full: false,
            invalid: false,
        }
    }
    fn push(&mut self, text: &str) {
        let start = self.total;
        self.total += text.len();
        if self.full || self.total <= self.offset {
            return;
        }
        let from = self.offset.saturating_sub(start);
        if !text.is_char_boundary(from) {
            self.invalid = true;
            return;
        }
        let end = text.floor_char_boundary((from + PAGE_BYTES - self.text.len()).min(text.len()));
        self.text.push_str(&text[from..end]);
        self.full = end < text.len() || self.text.len() == PAGE_BYTES;
    }
    pub(super) fn outcome(&mut self, outcome: &InvocationOutcome) {
        match outcome {
            InvocationOutcome::Failed { class, message } => self.push(&format!(
                "\nExecution failed ({class}): {}",
                message.as_deref().unwrap_or("no detail")
            )),
            InvocationOutcome::Cancelled { source } => {
                self.push(&format!("\nExecution cancelled: {source}"))
            }
            _ => {}
        }
    }
    pub(super) fn finish(self) -> Result<TextPage, Error> {
        if self.invalid || self.offset > self.total {
            return Err(Error::Invalid(
                "Invalid Graph result UTF-8 byte cursor".into(),
            ));
        }
        let next = self.offset + self.text.len();
        Ok(TextPage {
            text: self.text,
            offset: self.offset,
            total_bytes: self.total,
            next_offset: (next < self.total).then_some(next),
        })
    }
    pub(super) fn fact(fact: &Fact, offset: usize) -> Option<Self> {
        let mut text = Self::new(offset);
        match fact {
            Fact::ExecutorCompleted { text: value } => text.push(value),
            Fact::ModelCompleted { output, .. } => {
                for part in &output.parts {
                    if let ModelPart::Text {
                        text_kind: TextKind::Text,
                        text: value,
                        ..
                    } = part
                    {
                        text.push(value);
                    }
                }
                if text.total == 0 {
                    return None;
                }
            }
            Fact::ToolSettled { outcome, .. } => match outcome {
                ToolOutcome::Failed { message } => text.push(message),
                ToolOutcome::Succeeded {
                    model_projection, ..
                } => match model_projection {
                    DurableToolProjection::Text { text: value } => text.push(value),
                    DurableToolProjection::Json { value } => text.push(&value.to_string()),
                    DurableToolProjection::Content { parts } => {
                        for part in parts {
                            if let ProjectionPart::Text { text: value } = part {
                                text.push(value);
                            }
                        }
                    }
                    DurableToolProjection::Failure => text.push("Tool output unavailable"),
                },
            },
            _ => return None,
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_recover_segmented_unicode_and_escaped_text_without_retaining_full_results() {
        let segments = ["中😀\n\\\"\t".repeat(3000), "tail\n".repeat(4000)];
        let original = segments.concat();
        let mut offset = 0;
        let mut restored = String::new();
        loop {
            let mut window = Window::new(offset);
            for segment in &segments {
                window.push(segment);
            }
            let page = window.finish().unwrap();
            assert_eq!(page.total_bytes, original.len());
            assert!(serde_json::to_vec(&page).unwrap().len() < 32 * 1024);
            restored.push_str(&page.text);
            match page.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(restored, original);
        for offset in [1, 4, original.len() + 1] {
            let mut window = Window::new(offset);
            for segment in &segments {
                window.push(segment);
            }
            assert!(window.finish().is_err());
        }
    }
}
