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

use maka_plugins::session::history::Role;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct Query {
    #[schemars(length(min = 1, max = 8), inner(length(min = 1, max = 500)))]
    pub terms: Vec<String>,
    #[schemars(length(min = 1, max = 500))]
    pub question: Option<String>,
    #[schemars(range(min = 1, max = 25))]
    pub limit: Option<usize>,
    #[schemars(length(min = 1, max = 256))]
    pub session_id: Option<String>,
    pub since: Option<u64>,
    pub until: Option<u64>,
}
impl Query {
    pub fn validate(&mut self) -> Result<(), String> {
        if self.terms.is_empty()
            || self.terms.len() > 8
            || self.limit.is_some_and(|n| !(1..=25).contains(&n))
            || self.since.zip(self.until).is_some_and(|(a, b)| a > b)
            || self
                .question
                .as_ref()
                .is_some_and(|text| !bounded(text, 500))
            || self
                .session_id
                .as_ref()
                .is_some_and(|text| !bounded(text, 256))
        {
            return Err(
                "Recall requires 1–8 terms, a limit of 1–25 and valid Session/time bounds".into(),
            );
        }
        for term in &mut self.terms {
            if !bounded(term, 500) {
                return Err("Recall terms must contain 1–500 characters".into());
            }
            *term = term.trim().nfc().collect();
        }
        let mut seen = std::collections::BTreeSet::new();
        self.terms.retain(|term| seen.insert(fold(term)));
        Ok(())
    }
}
#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct More {
    #[schemars(length(min = 1, max = 256))]
    pub session_id: String,
    #[schemars(length(min = 1, max = 256))]
    pub anchor_message_id: String,
    #[schemars(range(max = 8))]
    pub before: Option<usize>,
    #[schemars(range(max = 8))]
    pub after: Option<usize>,
    /// Byte offset in the anchor's NFC text, useful for a long individual message.
    #[schemars(range(max = 67108864))]
    pub offset: Option<usize>,
}
impl More {
    pub fn validate(&self) -> Result<(), String> {
        if !bounded(&self.session_id, 256)
            || !bounded(&self.anchor_message_id, 256)
            || self.before.is_some_and(|v| v > 8)
            || self.after.is_some_and(|v| v > 8)
            || self.offset.is_some_and(|v| v > 64 * 1024 * 1024)
        {
            Err(
                "RecallMore requires an existing Session/message and 0–8 neighbors on each side"
                    .into(),
            )
        } else {
            Ok(())
        }
    }
}
fn bounded(text: &str, limit: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= limit
}
pub(super) fn fold(text: &str) -> String {
    text.nfc().collect::<String>().to_lowercase()
}

#[derive(Clone, Serialize)]
pub(super) struct PassageMessage {
    pub message_id: String,
    pub role: Role,
    pub text: String,
    pub timestamp: u64,
    pub is_anchor: bool,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub attachments: Vec<maka_runtime::attachment::AttachmentRef>,
}
#[derive(Serialize)]
pub(super) struct Passage {
    pub session_id: String,
    pub session_title: String,
    pub anchor_message_id: String,
    pub anchor_sequence: u64,
    pub turn_id: String,
    pub messages: Vec<PassageMessage>,
    pub matched_terms: Vec<String>,
    pub score: f64,
    pub has_more_before: bool,
    pub has_more_after: bool,
    pub truncated: bool,
}
#[derive(Serialize)]
pub(super) struct ResultSet {
    pub passages: Vec<Passage>,
    pub searched_every_session: bool,
    pub gaps: Vec<String>,
}
impl ResultSet {
    pub fn render(&self) -> String {
        let mut text = format!("Search complete: {}\n", self.searched_every_session);
        for gap in &self.gaps {
            text.push_str(&format!("{gap}\n"));
        }
        for passage in &self.passages {
            text.push_str(&format!(
                "\nSession: {} ({})\nAnchor: {}\nMore before: {}; more after: {}; clipped: {}\n",
                passage.session_title,
                passage.session_id,
                passage.anchor_message_id,
                passage.has_more_before,
                passage.has_more_after,
                passage.truncated
            ));
            for message in &passage.messages {
                text.push_str(&format!(
                    "\n{:?} {}{} (offset {}, next {:?}):\n{}\n",
                    message.role,
                    message.message_id,
                    if message.is_anchor { " [anchor]" } else { "" },
                    message.offset,
                    message.next_offset,
                    message.text
                ));
                for attachment in &message.attachments {
                    if let maka_runtime::attachment::StorageRef::SessionFile {
                        session_id,
                        relative_path,
                    } = &attachment.storage_ref
                    {
                        text.push_str(&format!(
                            "Attachment: {} ({}, {} bytes); session_id: {}; artifact_id: {}\n",
                            attachment.name,
                            attachment.mime_type,
                            attachment.bytes,
                            session_id,
                            relative_path
                        ));
                    }
                }
            }
        }
        text
    }
}
