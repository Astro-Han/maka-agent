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

mod action_id;
mod correction;
pub use action_id::ActionId;
mod description;
pub use correction::{
    CorrectionAbort, CorrectionIntent, CorrectionRequest, CorrectionTarget, correction_abort_source,
};
pub use description::{
    CreateDefaults, CreateExecution, CreateModel, CreateSpec, DelegationDescription,
};

pub const COORDINATION_SESSION_ID: &str = "maka_workhub_coordination";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopOutcome {
    CancelledPending,
    StopDelivered,
    AlreadyTerminal,
    NotOwned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopRequest {
    pub action_id: ActionId,
    pub request_fingerprint: String,
    pub source: Invocation,
    pub target_session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "StopResolutionWire", into = "StopResolutionWire")]
pub enum StopResolution {
    CancelledPending,
    StopDelivered { target_turn_id: String },
    AlreadyTerminal { target_turn_id: Option<String> },
    NotOwned { target_turn_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopIntent {
    pub request: StopRequest,
    pub delegation_action_id: ActionId,
    /// Frozen before cancellation; a retry never follows a later continuation.
    pub owner: Option<Invocation>,
}

impl StopRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        for id in [
            &self.source.session_id,
            &self.source.turn_id,
            &self.source.run_id,
            &self.source.invocation_id,
            &self.target_session_id,
        ] {
            crate::interaction::entity_id(id)?;
        }
        if self.source.session_id != COORDINATION_SESSION_ID
            || self.target_session_id == COORDINATION_SESSION_ID
            || !crate::archive::valid_projection_digest(&self.request_fingerprint)
        {
            return Err("invalid WorkHub stop authority");
        }
        Ok(())
    }
}

impl StopIntent {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.request.validate()?;
        if let Some(owner) = &self.owner {
            if owner.session_id != self.request.target_session_id {
                return Err("WorkHub stop owner belongs to another Session");
            }
            for id in [&owner.turn_id, &owner.run_id, &owner.invocation_id] {
                crate::interaction::entity_id(id)?;
            }
        }
        Ok(())
    }
}

impl StopResolution {
    pub fn outcome(&self) -> StopOutcome {
        match self {
            Self::CancelledPending => StopOutcome::CancelledPending,
            Self::StopDelivered { .. } => StopOutcome::StopDelivered,
            Self::AlreadyTerminal { .. } => StopOutcome::AlreadyTerminal,
            Self::NotOwned { .. } => StopOutcome::NotOwned,
        }
    }
    pub fn target_turn_id(&self) -> Option<&str> {
        match self {
            Self::CancelledPending => None,
            Self::AlreadyTerminal { target_turn_id } => target_turn_id.as_deref(),
            Self::StopDelivered { target_turn_id } | Self::NotOwned { target_turn_id } => {
                Some(target_turn_id)
            }
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Some(turn) = self.target_turn_id() {
            crate::interaction::entity_id(turn)?;
        }
        Ok(())
    }
}

// Keep the established durable shape, including explicit null for no Turn.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopResolutionWire {
    outcome: StopOutcome,
    target_turn_id: Option<String>,
}

impl From<StopResolution> for StopResolutionWire {
    fn from(value: StopResolution) -> Self {
        Self {
            outcome: value.outcome(),
            target_turn_id: value.target_turn_id().map(str::to_owned),
        }
    }
}

impl TryFrom<StopResolutionWire> for StopResolution {
    type Error = &'static str;
    fn try_from(value: StopResolutionWire) -> Result<Self, Self::Error> {
        let resolution = match (value.outcome, value.target_turn_id) {
            (StopOutcome::CancelledPending, None) => Self::CancelledPending,
            (StopOutcome::StopDelivered, Some(target_turn_id)) => {
                Self::StopDelivered { target_turn_id }
            }
            (StopOutcome::AlreadyTerminal, target_turn_id) => {
                Self::AlreadyTerminal { target_turn_id }
            }
            (StopOutcome::NotOwned, Some(target_turn_id)) => Self::NotOwned { target_turn_id },
            _ => return Err("invalid WorkHub stop resolution"),
        };
        resolution.validate()?;
        Ok(resolution)
    }
}

/// A resume action is owned by its real continuation opening, or by an
/// immutable observation that the delegated execution was already running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeOrigin {
    pub action_id: ActionId,
    pub request_fingerprint: String,
    pub coordinator: Invocation,
    pub delegation_action_id: ActionId,
}

impl ResumeOrigin {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.coordinator.session_id != COORDINATION_SESSION_ID
            || !crate::archive::valid_projection_digest(&self.request_fingerprint)
        {
            return Err("invalid WorkHub resume origin");
        }
        for id in [
            &self.coordinator.turn_id,
            &self.coordinator.run_id,
            &self.coordinator.invocation_id,
        ] {
            crate::interaction::entity_id(id)?;
        }
        Ok(())
    }
}

pub fn resumed_turn_id(action_id: &ActionId) -> String {
    let digest = crate::artifact::content_digest(format!("resume\0{action_id}").as_bytes());
    format!("wht_{}", &digest[7..55])
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delegation {
    pub action_id: ActionId,
    #[serde(default, skip_serializing_if = "DelegationKind::is_existing")]
    pub kind: DelegationKind,
    #[serde(default, skip_serializing_if = "DelegationDelivery::is_new_turn")]
    pub delivery: DelegationDelivery,
    pub request_fingerprint: String,
    /// Canonical user input, never text supplied by a model strategy.
    pub source_message_event_id: String,
    pub target: Invocation,
    /// Observed Session revision; new roots CAS it, steering CASes configuration
    /// because the existing worker legitimately advances this revision.
    pub target_revision: u64,
    pub delegation_text: String,
    /// Absent only in older facts that did not capture their display evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<DelegationDescription>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationKind {
    #[default]
    Existing,
    Created,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DelegationDelivery {
    #[default]
    NewTurn,
    Steering {
        configuration_digest: String,
    },
}

impl DelegationDelivery {
    fn is_new_turn(&self) -> bool {
        *self == Self::NewTurn
    }
    pub fn is_steering(&self) -> bool {
        matches!(self, Self::Steering { .. })
    }
}

impl DelegationKind {
    fn is_existing(&self) -> bool {
        *self == Self::Existing
    }
}

pub fn created_session_id(action_id: &ActionId) -> String {
    let digest = crate::artifact::content_digest(format!("create\0{action_id}").as_bytes());
    format!("whs_{}", &digest[7..55])
}

pub fn stop_abort_source(action_id: &ActionId) -> String {
    let digest = crate::artifact::content_digest(action_id.as_str().as_bytes());
    format!("workhub.direct_stop.{}", &digest[7..55])
}

impl Delegation {
    pub fn validate(&self, coordinator: &Invocation) -> Result<(), &'static str> {
        use crate::interaction::entity_id;
        if let Some(description) = &self.description {
            description.validate(self.kind)?;
        }
        if coordinator.session_id != COORDINATION_SESSION_ID
            || self.target.session_id == COORDINATION_SESSION_ID
        {
            return Err("invalid WorkHub delegation scope");
        }
        if self.kind == DelegationKind::Created
            && (self.target.session_id != created_session_id(&self.action_id)
                || self.target_revision != 1
                || self.delivery != DelegationDelivery::NewTurn)
        {
            return Err("invalid WorkHub creation identity");
        }
        for id in [
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
        if let DelegationDelivery::Steering {
            configuration_digest,
        } = &self.delivery
            && !crate::archive::valid_projection_digest(configuration_digest)
        {
            return Err("invalid WorkHub target configuration basis");
        }
        Ok(())
    }

    pub fn target_message_id(&self) -> String {
        format!(
            "workhub_{}",
            &crate::artifact::content_digest(self.action_id.as_str().as_bytes())[7..]
        )
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
                message_id: self.target_message_id(),
                submitted_content_digest: content
                    .content_digest()
                    .map_err(|_| "invalid delegated message")?,
                content,
            },
            submitted_placement: match &self.delivery {
                DelegationDelivery::NewTurn => Placement::NextTurn,
                DelegationDelivery::Steering { .. } => Placement::CurrentTurn,
            },
            disposition: match &self.delivery {
                DelegationDelivery::NewTurn => MessageDisposition::TurnStarted,
                DelegationDelivery::Steering { .. } => MessageDisposition::Steering,
            },
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
