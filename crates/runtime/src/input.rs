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

use crate::attachment::AttachmentRef;
use serde::{Deserialize, Serialize};
mod references;
pub use references::{DirectoryReference, InlineReference, InlineReferenceKind, QuoteRef};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvocationInput {
    Handoff {
        claim: Box<crate::continuation::ContinuationClaim>,
        pause: Box<crate::handoff::HandoffPause>,
    },
    Continuation {
        claim: Box<crate::continuation::ContinuationClaim>,
        request_fingerprint: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workhub_resume: Option<crate::workhub::ResumeOrigin>,
    },
    ContextCompact {
        request_fingerprint: String,
    },
    Message {
        content: MessageInput,
        request_fingerprint: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        source_messages: Vec<crate::message::RootSourceMessage>,
        /// Legacy turn.start has no Message identity; its receipt belongs to this opening.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skill_invocation: Option<Box<crate::skills::SkillInvocationResult>>,
    },
    Code {
        source: String,
    },
}

impl InvocationInput {
    pub fn request_fingerprint(&self) -> Option<&str> {
        match self {
            Self::Message {
                request_fingerprint,
                ..
            } => request_fingerprint.as_deref(),
            Self::Continuation {
                request_fingerprint,
                ..
            }
            | Self::ContextCompact {
                request_fingerprint,
            } => Some(request_fingerprint),
            Self::Handoff { .. } | Self::Code { .. } => None,
        }
    }

    pub fn inherited_claim(&self) -> Option<&crate::continuation::ContinuationClaim> {
        match self {
            Self::Continuation { claim, .. } | Self::Handoff { claim, .. } => Some(claim),
            _ => None,
        }
    }

    pub fn validate_inheritance(
        &self,
        target: &crate::event::Invocation,
    ) -> Result<(), &'static str> {
        match self {
            Self::Continuation { claim, .. } => claim.validate(target),
            Self::Handoff { claim, pause } => pause.validate_claim(claim, target),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageInput {
    pub text: String,
    pub display_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quotes: Option<Vec<QuoteRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_references: Option<Vec<DirectoryReference>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_references: Option<Vec<InlineReference>>,
}

impl MessageInput {
    /// Opaque Rust-host admission identity; never derived from a UI projection.
    pub fn content_digest(&self) -> Result<String, serde_json::Error> {
        use sha2::{Digest, Sha256};
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(self)?)
        ))
    }
    /// All retained text, including reference metadata; used before projection clones.
    pub fn text_bytes(&self) -> usize {
        let bytes = std::iter::once(self.text.as_str())
            .chain(self.display_text.as_deref())
            .chain(self.quotes.iter().flatten().flat_map(|quote| {
                std::iter::once(quote.text.as_str())
                    .chain(quote.label.as_deref())
                    .chain(quote.source_turn_id.as_deref())
            }))
            .chain(
                self.directory_references
                    .iter()
                    .flatten()
                    .flat_map(|reference| [reference.host_id.as_str(), reference.path.as_str()]),
            )
            .chain(
                self.inline_references
                    .iter()
                    .flatten()
                    .flat_map(|reference| [reference.value.as_str(), reference.label.as_str()]),
            )
            .fold(0usize, |bytes, text| bytes.saturating_add(text.len()));
        self.attachments
            .iter()
            .flatten()
            .fold(bytes, |bytes, attachment| {
                bytes.saturating_add(attachment.text_bytes())
            })
    }
}

/// Immutable client identity and prepared content delivered by a root or steering fact.
/// Its submitted digest identifies client content before Host preparation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveredMessage {
    pub message_id: String,
    pub content: MessageInput,
    pub submitted_content_digest: String,
}

impl DeliveredMessage {
    pub fn validate(&self) -> Result<(), &'static str> {
        crate::interaction::entity_id(&self.message_id)?;
        if !crate::archive::valid_projection_digest(&self.submitted_content_digest) {
            return Err("invalid submitted message digest");
        }
        if self.content.text_bytes() > 64 * 1024
            || serde_json::to_vec(self)
                .map_err(|_| "invalid steering message")?
                .len()
                > 1024 * 1024
        {
            return Err("steering message exceeds durable capacity");
        }
        Ok(())
    }
}

impl From<String> for MessageInput {
    fn from(text: String) -> Self {
        Self {
            text,
            display_text: None,
            attachments: None,
            quotes: None,
            directory_references: None,
            inline_references: None,
        }
    }
}

impl From<&str> for MessageInput {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}
