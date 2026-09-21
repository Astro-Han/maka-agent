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

//! Outbound live queue rows reuse the currently supported message content.
//! Host-owned `session_context` attachment references are not represented by
//! that content type yet; this is not a decoder for arbitrary existing queues.
use super::super::{ensure, entity, id};
use super::{count, encoded};
use crate::{Result, turn::MessageContent};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageQueueProjection {
    pub host_epoch: String,
    pub queue_revision: u64,
    pub steering: Vec<SteeringMessageSnapshot>,
    pub followup: Vec<FollowupMessageSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueMessage {
    pub entry_id: String,
    pub message_id: String,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SteeringState {
    Queued,
    InFlight,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SteeringMessageSnapshot {
    pub message: QueueMessage,
    pub state: SteeringState,
}
#[derive(Debug, Clone, PartialEq)]
pub struct FollowupMessageSnapshot {
    pub message: QueueMessage,
}
impl SteeringMessageSnapshot {
    pub fn new(message: QueueMessage, state: SteeringState) -> Self {
        Self { message, state }
    }
}
impl FollowupMessageSnapshot {
    pub fn new(message: QueueMessage) -> Self {
        Self { message }
    }
}
#[derive(Serialize)]
struct QueueMessageWire<'a> {
    #[serde(flatten)]
    message: &'a QueueMessage,
    state: SteeringState,
    placement: &'static str,
}
impl Serialize for SteeringMessageSnapshot {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        QueueMessageWire {
            message: &self.message,
            state: self.state,
            placement: "current_turn",
        }
        .serialize(serializer)
    }
}
impl Serialize for FollowupMessageSnapshot {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        QueueMessageWire {
            message: &self.message,
            state: SteeringState::Queued,
            placement: "next_turn",
        }
        .serialize(serializer)
    }
}
impl SessionMessageQueueProjection {
    pub fn validate(&self) -> Result<()> {
        id(&self.host_epoch)?;
        count(self.queue_revision, false)?;
        ensure(
            self.steering.len() + self.followup.len() <= 64,
            "Too many queue entries",
        )?;
        let mut entries = HashSet::new();
        let mut messages = HashSet::new();
        for message in self
            .steering
            .iter()
            .map(|s| &s.message)
            .chain(self.followup.iter().map(|s| &s.message))
        {
            entity(&message.entry_id)?;
            entity(&message.message_id)?;
            ensure(
                entries.insert(&message.entry_id) && messages.insert(&message.message_id),
                "Duplicate queue identity",
            )?;
            message.content.clone().validate(false)?;
        }
        encoded(self, 52 * 1024)
    }
}
