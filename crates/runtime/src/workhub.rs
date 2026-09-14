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

//! Durable WorkHub delegation identities; execution remains ordinary queued work.
use crate::{
    attachment::{AttachmentRef, StorageRef},
    event::Invocation,
    input::{DeliveredMessage, MessageInput},
    message::{MessageDisposition, Placement, RootSourceMessage},
};
use serde::{Deserialize, Serialize};

pub const COORDINATION_SESSION_ID: &str = "maka_workhub_coordination";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delegation {
    pub action_id: String,
    pub request_fingerprint: String,
    /// Canonical user input, never text supplied by a model strategy.
    pub source_message_event_id: String,
    pub target: Invocation,
    /// Candidate metadata observed by admission, checked with the pending message commit.
    pub target_revision: u64,
    pub delegation_text: String,
}

impl Delegation {
    pub fn validate(&self, coordinator: &Invocation) -> Result<(), &'static str> {
        use crate::interaction::entity_id;
        if coordinator.session_id != COORDINATION_SESSION_ID
            || self.target.session_id == COORDINATION_SESSION_ID
        {
            return Err("invalid WorkHub delegation scope");
        }
        for id in [
            &self.action_id,
            &self.source_message_event_id,
            &self.target.session_id,
            &self.target.turn_id,
            &self.target.run_id,
            &self.target.invocation_id,
        ] {
            entity_id(id).map_err(|_| "invalid WorkHub delegation identity")?;
        }
        if !(1..=9_007_199_254_740_991).contains(&self.target_revision)
            || !crate::archive::valid_projection_digest(&self.request_fingerprint)
            || self.delegation_text.trim().is_empty()
            || self.delegation_text.len() > 48 * 1024
        {
            return Err("invalid WorkHub delegation content");
        }
        Ok(())
    }

    /// Derive the sole target message from its canonical source and recorded task.
    pub fn message(&self, user: &MessageInput) -> Result<RootSourceMessage, &'static str> {
        let content = MessageInput {
            text: format!(
                "User request:\n{}\n\nDelegated task:\n{}",
                user.text, self.delegation_text
            ),
            display_text: None,
            attachments: user
                .attachments
                .as_ref()
                .map(|items| {
                    items
                        .iter()
                        .map(|attachment| self.attachment(attachment))
                        .collect()
                })
                .transpose()?,
            ..user.clone()
        };
        if content.text_bytes() > 64 * 1024 {
            return Err("delegated message exceeds durable capacity");
        }
        let message = RootSourceMessage {
            message: DeliveredMessage {
                message_id: format!(
                    "workhub_{}",
                    &crate::artifact::content_digest(self.action_id.as_bytes())[7..]
                ),
                submitted_content_digest: content
                    .content_digest()
                    .map_err(|_| "invalid delegated message")?,
                content,
            },
            submitted_placement: Placement::NextTurn,
            disposition: MessageDisposition::TurnStarted,
            skill_invocation: Default::default(),
            submitted_intent: None,
        };
        message.validate()?;
        Ok(message)
    }

    /// Stable destination; only the canonical coordination message supplies sources.
    pub fn attachment(&self, source: &AttachmentRef) -> Result<AttachmentRef, &'static str> {
        let StorageRef::SessionFile {
            session_id,
            relative_path,
        } = &source.storage_ref
        else {
            return Err("WorkHub attachments require Session Artifact references");
        };
        if session_id != COORDINATION_SESSION_ID {
            return Err("WorkHub attachment belongs to another Session");
        }
        crate::interaction::entity_id(relative_path)?;
        Ok(AttachmentRef {
            storage_ref: StorageRef::SessionFile {
                session_id: self.target.session_id.clone(),
                relative_path: crate::artifact::upload_artifact_id(
                    &self.target.session_id,
                    &format!("workhub:{}:{relative_path}", self.action_id),
                ),
            },
            ..source.clone()
        })
    }
}
