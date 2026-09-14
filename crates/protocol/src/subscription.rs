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

//! Epoch 141 subscription inputs and assistant observation boundary.
//! Outbound snapshots additionally support the host's empty interaction state
//! and transcript-none policy; they do not claim full snapshot decoding support.
//! JSON callers must use the decode functions for semantic validation.
mod resource;
mod snapshot;
pub use resource::*;
mod tool;
use crate::{
    ProtocolError, Result, codec,
    turn::{TurnSnapshot, TurnState},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
pub use snapshot::*;
use std::collections::HashSet;
pub use tool::*;

pub const SESSION_TRANSCRIPT_BOOTSTRAP_MAX_BYTES: u64 = 16_384;
pub const SESSION_LIVE_DELTA_MAX_BYTES: usize = 16 * 1024;
pub const SUBSCRIPTION_FRAME_MAX_BYTES: usize = 65_535;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionOpenInput {
    pub session_id: String,
    pub transcript: TranscriptPolicy,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TranscriptPolicy {
    None,
    Tail { max_bytes: u64 },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionCloseInput {
    pub subscription_id: String,
}
pub type SubscriptionCloseResult = SubscriptionCloseInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantStreamKind {
    Text,
    Thinking,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionAssistantStreamIdentity {
    pub kind: AssistantStreamKind,
    pub turn_id: String,
    pub message_id: String,
}

/// The wire permits `true` or omission, never false or null.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "bool", into = "bool")]
pub struct TrueFlag;
impl TryFrom<bool> for TrueFlag {
    type Error = &'static str;
    fn try_from(value: bool) -> std::result::Result<Self, Self::Error> {
        if value {
            Ok(Self)
        } else {
            Err("Flag must be true")
        }
    }
}
impl From<TrueFlag> for bool {
    fn from(_: TrueFlag) -> Self {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionAssistantDelta {
    pub kind: AssistantStreamKind,
    pub turn_id: String,
    pub run_id: String,
    pub message_id: String,
    /// Offset measured in JavaScript UTF-16 code units, not UTF-8 bytes.
    pub start_offset: u64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset: Option<TrueFlag>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete: Option<TrueFlag>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupted: Option<TrueFlag>,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionClosedReason {
    SlowConsumer,
    SessionRemoved,
    AccessRevoked,
}
/// Only the delta and closure subset; not a decoder for all subscription frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum AssistantObservationFrame {
    #[serde(rename = "subscription.session_delta")]
    SessionDelta {
        host_epoch: String,
        subscription_id: String,
        sequence: u64,
        session_id: String,
        delta: SessionAssistantDelta,
    },
    #[serde(rename = "subscription.closed")]
    Closed {
        host_epoch: String,
        subscription_id: String,
        sequence: u64,
        reason: SubscriptionClosedReason,
    },
}

pub fn decode_subscription_open_input(value: &Value) -> Result<SubscriptionOpenInput> {
    let input: SubscriptionOpenInput = decode(value)?;
    entity(&input.session_id)?;
    let policy = codec::record(&value["transcript"], "transcript policy")?;
    codec::exact(
        policy,
        match input.transcript {
            TranscriptPolicy::None => &["kind"],
            TranscriptPolicy::Tail { .. } => &["kind", "maxBytes"],
        },
    )?;
    if let TranscriptPolicy::Tail { max_bytes } = input.transcript {
        ensure(
            (2..=SESSION_TRANSCRIPT_BOOTSTRAP_MAX_BYTES).contains(&max_bytes),
            "Invalid transcript byte limit",
        )?;
    }
    Ok(input)
}
pub fn decode_subscription_close_input(value: &Value) -> Result<SubscriptionCloseInput> {
    let input: SubscriptionCloseInput = decode(value)?;
    id(&input.subscription_id)?;
    Ok(input)
}
pub fn decode_subscription_close_result(value: &Value) -> Result<SubscriptionCloseResult> {
    decode_subscription_close_input(value)
}
pub fn decode_active_assistant_streams(
    value: &Value,
    root_turn: Option<&TurnSnapshot>,
) -> Result<Vec<SessionAssistantStreamIdentity>> {
    let streams: Vec<SessionAssistantStreamIdentity> = decode(value)?;
    let mut keys = HashSet::new();
    for stream in &streams {
        entity(&stream.turn_id)?;
        entity(&stream.message_id)?;
        ensure(
            root_turn.is_some_and(|root| {
                root.turn_id == stream.turn_id
                    && matches!(
                        root.state,
                        TurnState::Admitted(_)
                            | TurnState::Created(_)
                            | TurnState::Running(_)
                            | TurnState::WaitingForUser(_)
                    )
            }),
            "Active assistant stream has no active root Turn",
        )?;
        ensure(
            keys.insert((stream.kind, &stream.message_id)),
            "Duplicate assistant stream",
        )?;
    }
    Ok(streams)
}
pub fn decode_assistant_delta(value: &Value) -> Result<SessionAssistantDelta> {
    let delta: SessionAssistantDelta = decode(value)?;
    validate_delta(&delta)?;
    Ok(delta)
}
pub fn decode_assistant_observation_frame(value: &Value) -> Result<AssistantObservationFrame> {
    ensure(
        serde_json::to_vec(value)
            .map_err(|e| ProtocolError::invalid(e.to_string()))?
            .len()
            <= SUBSCRIPTION_FRAME_MAX_BYTES,
        "Subscription frame exceeds byte limit",
    )?;
    let frame: AssistantObservationFrame = decode(value)?;
    let (epoch, subscription, sequence) = match &frame {
        AssistantObservationFrame::SessionDelta {
            host_epoch,
            subscription_id,
            sequence,
            session_id,
            delta,
        } => {
            entity(session_id)?;
            validate_delta(delta)?;
            (host_epoch, subscription_id, sequence)
        }
        AssistantObservationFrame::Closed {
            host_epoch,
            subscription_id,
            sequence,
            ..
        } => (host_epoch, subscription_id, sequence),
    };
    id(epoch)?;
    id(subscription)?;
    ensure(*sequence > 0, "Invalid subscription sequence")?;
    Ok(frame)
}
fn validate_delta(delta: &SessionAssistantDelta) -> Result<()> {
    entity(&delta.turn_id)?;
    entity(&delta.run_id)?;
    entity(&delta.message_id)?;
    ensure(
        delta.interrupted.is_none() || delta.complete.is_some(),
        "Interrupted assistant delta must be complete",
    )?;
    ensure(
        delta.reset.is_none() || delta.start_offset == 0,
        "Reset must start at offset zero",
    )?;
    ensure(
        (delta.complete.is_some() || !delta.text.is_empty())
            && delta.text.len() <= SESSION_LIVE_DELTA_MAX_BYTES,
        "Invalid assistant delta text",
    )
}
fn entity(value: &str) -> Result<()> {
    ensure(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "Invalid entity ID",
    )
}
fn id(value: &str) -> Result<()> {
    ensure(
        !value.is_empty() && value.encode_utf16().count() <= 128,
        "Invalid ID",
    )
}
fn ensure(valid: bool, message: &str) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::invalid(message))
    }
}
// This bounded slice has no nullable fields. Normalize Number.isSafeInteger
// spellings (1e0/1.0) before deserializing integers, and reject optional nulls.
fn normalize(value: &Value) -> Result<Value> {
    match value {
        Value::Null => Err(ProtocolError::invalid("Unexpected null")),
        Value::Number(_) => Ok(Value::from(codec::count(value, "count")?)),
        Value::Array(a) => Ok(Value::Array(
            a.iter().map(normalize).collect::<Result<_>>()?,
        )),
        Value::Object(o) => Ok(Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), normalize(v)?)))
                .collect::<Result<_>>()?,
        )),
        _ => Ok(value.clone()),
    }
}
fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(normalize(value)?).map_err(|e| ProtocolError::invalid(e.to_string()))
}
