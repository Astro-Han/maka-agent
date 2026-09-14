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

use super::{MAX_ENTRIES, MAX_PROJECTION_BYTES, encoded, ensure, epoch};
use crate::{
    Result,
    turn::{self, MessageContent},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub use maka_runtime::message::Placement;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    Queued,
    InFlight,
    Retracted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueEntry {
    pub entry_id: String,
    pub message_id: String,
    pub content: MessageContent,
    pub placement: Placement,
    pub state: EntryState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueProjection {
    pub host_epoch: String,
    pub queue_revision: u64,
    pub steering: Vec<QueueEntry>,
    pub followup: Vec<QueueEntry>,
}

pub fn decode_queue_projection(value: &Value) -> Result<QueueProjection> {
    let mut queue: QueueProjection = turn::decode(value)?;
    epoch(&queue.host_epoch)?;
    entries(queue.steering.iter_mut().chain(&mut queue.followup))?;
    ensure(
        queue
            .steering
            .iter()
            .all(|e| e.placement == Placement::CurrentTurn && e.state != EntryState::Retracted),
        "Invalid steering entry",
    )?;
    ensure(
        queue
            .followup
            .iter()
            .all(|e| e.placement == Placement::NextTurn && e.state == EntryState::Queued),
        "Invalid followup entry",
    )?;
    encoded(&queue, MAX_PROJECTION_BYTES)?;
    Ok(queue)
}

pub(super) fn retracted(entries: &mut [QueueEntry]) -> Result<()> {
    self::entries(entries.iter_mut())?;
    ensure(
        entries.iter().all(|e| e.state == EntryState::Retracted),
        "Invalid retracted state",
    )
}

fn entries<'a>(entries: impl Iterator<Item = &'a mut QueueEntry>) -> Result<()> {
    let mut entry_ids = HashSet::new();
    let mut message_ids = HashSet::new();
    for entry in entries {
        turn::entity(&entry.entry_id)?;
        turn::entity(&entry.message_id)?;
        entry.content.validate_admission(false)?;
        ensure(
            entry.state != EntryState::InFlight || entry.placement == Placement::CurrentTurn,
            "Invalid in-flight placement",
        )?;
        ensure(
            entry_ids.insert(entry.entry_id.clone())
                && message_ids.insert(entry.message_id.clone()),
            "Duplicate queue identity",
        )?;
        ensure(entry_ids.len() <= MAX_ENTRIES, "Too many queue entries")?;
    }
    Ok(())
}
