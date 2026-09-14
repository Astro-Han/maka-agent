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

use super::{bytes, encoded, ensure, entity, text};
use crate::Result;
use maka_runtime::attachment::canonical_relative_path as relative;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageContent {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory_references: Option<Vec<DirectoryReference>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quotes: Option<Vec<QuoteRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_references: Option<Vec<InlineReference>>,
}
pub use maka_runtime::attachment::{AttachmentKind, AttachmentRef, StorageRef};
pub use maka_runtime::input::{DirectoryReference, InlineReference, InlineReferenceKind, QuoteRef};
impl MessageContent {
    pub fn validate_admission(&mut self, allow_empty: bool) -> Result<()> {
        self.validate(true)?;
        ensure(
            allow_empty
                || !self.text.is_empty()
                || self.quotes.is_some()
                || self.attachments.is_some()
                || self.directory_references.is_some(),
            "Empty message content",
        )?;
        ensure(
            !self.attachments.iter().flatten().any(|attachment| {
                matches!(attachment.storage_ref, StorageRef::SessionContext { .. })
            }),
            "Session context references are Host-owned",
        )
    }

    pub(crate) fn validate(&mut self, allow_empty: bool) -> Result<()> {
        bytes(&self.text, 48 * 1024, allow_empty)?;
        if self.display_text.as_ref() == Some(&self.text) {
            self.display_text = None;
        }
        if let Some(display) = &self.display_text {
            bytes(display, 48 * 1024, true)?;
        }
        if let Some(refs) = &self.directory_references {
            ensure(refs.len() <= 4, "Too many directory references")?;
            for reference in refs {
                entity(&reference.host_id)?;
                text(&reference.path, 4096)?;
                ensure(absolute(&reference.path), "Invalid directory path")?;
            }
        }
        if let Some(attachments) = &self.attachments {
            ensure(attachments.len() <= 8, "Too many attachments")?;
            for attachment in attachments {
                bytes(&attachment.name, 512, false)?;
                bytes(&attachment.mime_type, 256, false)?;
                ensure(
                    attachment.bytes <= 50 * 1024 * 1024,
                    "Attachment exceeds byte limit",
                )?;
                match &attachment.storage_ref {
                    StorageRef::SessionContext { session_id, ref_id } => {
                        entity(session_id)?;
                        bytes(ref_id, 4096, false)?;
                        ensure(ref_id.chars().count() <= 512, "Invalid context reference")?;
                    }
                    StorageRef::SessionFile {
                        session_id,
                        relative_path,
                    } => {
                        entity(session_id)?;
                        bytes(relative_path, 4096, false)?;
                        ensure(relative(relative_path), "Invalid relative path")?;
                    }
                    StorageRef::WorkspaceFile { relative_path } => {
                        bytes(relative_path, 4096, false)?;
                        ensure(relative(relative_path), "Invalid relative path")?;
                    }
                    StorageRef::ExternalFile { absolute_path } => {
                        bytes(absolute_path, 4096, false)?;
                        ensure(absolute(absolute_path), "Invalid absolute path")?;
                    }
                }
            }
        }
        if let Some(quotes) = &self.quotes {
            ensure(quotes.len() <= 16, "Too many quotes")?;
            for quote in quotes {
                text(&quote.text, 32000)?;
                if let Some(label) = &quote.label {
                    text(label, 200)?;
                }
                if let Some(id) = &quote.source_turn_id {
                    entity(id)?;
                }
            }
        }
        if let Some(refs) = &self.inline_references {
            ensure(refs.len() <= 32, "Too many inline references")?;
            let visible: Vec<u16> = self
                .display_text
                .as_deref()
                .unwrap_or(&self.text)
                .encode_utf16()
                .collect();
            let mut end = 0usize;
            for reference in refs {
                text(&reference.value, 4096)?;
                text(&reference.label, 200)?;
                let token: Vec<u16> = reference.value.encode_utf16().collect();
                let start = usize::try_from(reference.start).unwrap_or(usize::MAX);
                let next = start.checked_add(token.len());
                ensure(
                    start >= end
                        && next.is_some_and(|n| visible.get(start..n) == Some(token.as_slice())),
                    "Inline reference does not match text",
                )?;
                end = next.unwrap();
                let valid = match reference.kind {
                    InlineReferenceKind::Skill => {
                        reference.value.strip_prefix("/skill:").is_some_and(|s| {
                            !s.is_empty()
                                && s.bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                        })
                    }
                    InlineReferenceKind::WorkspaceFile => {
                        reference.value.strip_prefix('@').is_some_and(relative)
                    }
                };
                ensure(valid, "Invalid inline reference token")?;
            }
        }
        omit_empty(&mut self.attachments);
        omit_empty(&mut self.directory_references);
        omit_empty(&mut self.quotes);
        encoded(self, 52 * 1024)
    }
}
impl From<maka_runtime::input::MessageInput> for MessageContent {
    fn from(content: maka_runtime::input::MessageInput) -> Self {
        Self {
            text: content.text,
            display_text: content.display_text,
            attachments: content.attachments,
            directory_references: content.directory_references,
            quotes: content.quotes,
            inline_references: content.inline_references,
        }
    }
}
impl From<MessageContent> for maka_runtime::input::MessageInput {
    fn from(content: MessageContent) -> Self {
        Self {
            text: content.text,
            display_text: content.display_text,
            attachments: content.attachments,
            directory_references: content.directory_references,
            quotes: content.quotes,
            inline_references: content.inline_references,
        }
    }
}
fn omit_empty<T>(value: &mut Option<Vec<T>>) {
    if value.as_ref().is_some_and(Vec::is_empty) {
        *value = None;
    }
}
fn absolute(path: &str) -> bool {
    if path.is_empty() || path.contains('\0') {
        return false;
    }
    let b = path.as_bytes();
    if path.starts_with('/')
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b"/\\".contains(&b[2]))
    {
        return true;
    }
    path.strip_prefix("\\\\").is_some_and(|p| {
        let mut parts = p.split(['/', '\\']);
        parts.next().is_some_and(|s| !s.is_empty()) && parts.next().is_some_and(|s| !s.is_empty())
    })
}
