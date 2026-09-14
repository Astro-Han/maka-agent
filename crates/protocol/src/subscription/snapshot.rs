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

//! Outbound schema-5 observation slice, including ClientCapability interactions.
//! This is deliberately not a full snapshot decoder.
#[path = "goal.rs"]
mod goal;
#[path = "queue.rs"]
mod queue;
use super::{
    SUBSCRIPTION_FRAME_MAX_BYTES, SessionAssistantStreamIdentity, SubscriptionOpenInput,
    TranscriptPolicy, decode_active_assistant_streams, ensure, entity, id,
};
pub use crate::interaction::SessionInteractionProjection;
use crate::transcript::{SessionTranscriptBootstrap, validate_bootstrap_for_input};
use crate::{
    ProtocolError, Result,
    session::SessionStatus,
    turn::{TurnSnapshot, decode_turn_snapshot},
};
pub use goal::*;
pub use queue::*;
use serde::{Serialize, Serializer, ser::SerializeStruct};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionObservationIdentity {
    pub session_id: String,
    pub metadata_revision: u64,
    pub status: SessionStatus,
    pub created_at: u64,
    pub is_archived: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionObservationSnapshot {
    pub session: SessionObservationIdentity,
    pub projection_revision: u64,
    pub root_turn: Option<TurnSnapshot>,
    pub goal: Option<GoalProjection>,
    pub queue: SessionMessageQueueProjection,
    pub interactions: SessionInteractionProjection,
}
impl Serialize for SessionObservationSnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut snapshot = serializer.serialize_struct("SessionObservationSnapshot", 7)?;
        snapshot.serialize_field("schemaVersion", &5_u8)?;
        snapshot.serialize_field("session", &self.session)?;
        snapshot.serialize_field("projectionRevision", &self.projection_revision)?;
        snapshot.serialize_field("rootTurn", &self.root_turn)?;
        snapshot.serialize_field("goal", &self.goal)?;
        snapshot.serialize_field("queue", &self.queue)?;
        snapshot.serialize_field("interactions", &self.interactions)?;
        snapshot.end()
    }
}
impl SessionObservationSnapshot {
    pub fn new(
        session: SessionObservationIdentity,
        projection_revision: u64,
        root_turn: Option<TurnSnapshot>,
        goal: Option<GoalProjection>,
        queue: SessionMessageQueueProjection,
        interactions: SessionInteractionProjection,
    ) -> Self {
        Self {
            session,
            projection_revision,
            root_turn,
            goal,
            queue,
            interactions,
        }
    }
    pub fn validate(&self) -> Result<()> {
        entity(&self.session.session_id)?;
        count(self.session.metadata_revision, true)?;
        count(self.session.created_at, false)?;
        count(self.projection_revision, true)?;
        if let Some(turn) = &self.root_turn {
            ensure(
                turn.session_id == self.session.session_id,
                "Root Turn belongs to another Session",
            )?;
            decode_turn_snapshot(&serde_json::to_value(turn).map_err(invalid)?)?;
        }
        if let Some(goal) = &self.goal {
            ensure(
                goal.session_id == self.session.session_id,
                "Goal belongs to another Session",
            )?;
            goal.validate()?;
        }
        self.queue.validate()?;
        self.interactions.validate(&self.session.session_id)?;
        encoded(self, 56 * 1024)
    }
}

/// Outbound success with a factual frozen bootstrap or no transcript requested.
#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionOpenResult {
    pub host_epoch: String,
    pub subscription_id: String,
    pub next_sequence: u64,
    pub snapshot: SessionObservationSnapshot,
    pub active_assistant_streams: Vec<SessionAssistantStreamIdentity>,
    pub transcript: Option<SessionTranscriptBootstrap>,
}
impl Serialize for SubscriptionOpenResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut result = serializer.serialize_struct("SubscriptionOpenResult", 6)?;
        result.serialize_field("hostEpoch", &self.host_epoch)?;
        result.serialize_field("subscriptionId", &self.subscription_id)?;
        result.serialize_field("nextSequence", &self.next_sequence)?;
        result.serialize_field("snapshot", &self.snapshot)?;
        result.serialize_field("activeAssistantStreams", &self.active_assistant_streams)?;
        result.serialize_field("transcript", &self.transcript)?;
        result.end()
    }
}
impl SubscriptionOpenResult {
    pub fn new(
        host_epoch: String,
        subscription_id: String,
        next_sequence: u64,
        snapshot: SessionObservationSnapshot,
        active_assistant_streams: Vec<SessionAssistantStreamIdentity>,
        transcript: Option<SessionTranscriptBootstrap>,
    ) -> Self {
        Self {
            host_epoch,
            subscription_id,
            next_sequence,
            snapshot,
            active_assistant_streams,
            transcript,
        }
    }
    pub fn validate_for(
        &self,
        input: &SubscriptionOpenInput,
        negotiated_epoch: &str,
    ) -> Result<()> {
        match (&input.transcript, &self.transcript) {
            (TranscriptPolicy::None, None) => {}
            (TranscriptPolicy::Tail { max_bytes }, Some(bootstrap)) => {
                validate_bootstrap_for_input(bootstrap, &input.session_id, *max_bytes)?
            }
            _ => {
                return Err(ProtocolError::invalid(
                    "Transcript does not match requested policy",
                ));
            }
        }
        id(&self.host_epoch)?;
        id(&self.subscription_id)?;
        count(self.next_sequence, true)?;
        self.snapshot.validate()?;
        ensure(
            self.host_epoch == negotiated_epoch
                && self.snapshot.queue.host_epoch == self.host_epoch,
            "Subscription epoch mismatch",
        )?;
        ensure(
            self.snapshot.session.session_id == input.session_id,
            "Subscription Session mismatch",
        )?;
        decode_active_assistant_streams(
            &serde_json::to_value(&self.active_assistant_streams).map_err(invalid)?,
            self.snapshot.root_turn.as_ref(),
        )?;
        encoded(self, 92 * 1024)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum SessionProjectionFrame {
    #[serde(rename = "subscription.session_projection")]
    SessionProjection {
        host_epoch: String,
        subscription_id: String,
        sequence: u64,
        snapshot: SessionObservationSnapshot,
    },
}
impl SessionProjectionFrame {
    pub fn validate(&self) -> Result<()> {
        let Self::SessionProjection {
            host_epoch,
            subscription_id,
            sequence,
            snapshot,
        } = self;
        id(host_epoch)?;
        id(subscription_id)?;
        count(*sequence, true)?;
        snapshot.validate()?;
        ensure(
            snapshot.queue.host_epoch == *host_epoch,
            "Projection queue epoch mismatch",
        )?;
        encoded(self, SUBSCRIPTION_FRAME_MAX_BYTES)
    }
}
fn count(value: u64, positive: bool) -> Result<()> {
    ensure(
        value <= 9_007_199_254_740_991 && (!positive || value > 0),
        "Invalid safe integer",
    )
}
fn encoded<T: Serialize>(value: &T, max: usize) -> Result<()> {
    ensure(
        serde_json::to_vec(value).map_err(invalid)?.len() <= max,
        "Observation exceeds byte limit",
    )
}
fn invalid(error: serde_json::Error) -> ProtocolError {
    ProtocolError::invalid(error.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum TranscriptAdvancedFrame {
    #[serde(rename = "subscription.transcript_advanced")]
    TranscriptAdvanced {
        host_epoch: String,
        subscription_id: String,
        sequence: u64,
        session_id: String,
        through_sequence: u64,
    },
}
impl TranscriptAdvancedFrame {
    pub fn validate(&self) -> Result<()> {
        let Self::TranscriptAdvanced {
            host_epoch,
            subscription_id,
            sequence,
            session_id,
            through_sequence,
        } = self;
        id(host_epoch)?;
        id(subscription_id)?;
        entity(session_id)?;
        count(*sequence, true)?;
        count(*through_sequence, false)
    }
}
