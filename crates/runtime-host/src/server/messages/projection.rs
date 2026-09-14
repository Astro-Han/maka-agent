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

use maka_event_log::message_queue::MessageQueue;
use maka_protocol::{message::*, turn::MessageContent};
use maka_runtime::message::MessageDisposition;

pub(crate) fn project(epoch: &str, queue: &MessageQueue) -> QueueProjection {
    let mut projection = QueueProjection {
        host_epoch: epoch.into(),
        queue_revision: queue.revision,
        steering: Vec::new(),
        followup: Vec::new(),
    };
    for entry in &queue.entries {
        let source = &entry.source;
        let placement = match source.disposition {
            MessageDisposition::TurnStarted => continue,
            MessageDisposition::Steering => Placement::CurrentTurn,
            MessageDisposition::Followup => Placement::NextTurn,
        };
        let row = QueueEntry {
            entry_id: source.message.message_id.clone(),
            message_id: source.message.message_id.clone(),
            content: MessageContent::from(source.message.content.clone()),
            placement,
            state: EntryState::Queued,
        };
        match placement {
            Placement::CurrentTurn => projection.steering.push(row),
            Placement::NextTurn => projection.followup.push(row),
        }
    }
    projection
}

pub(crate) fn subscription(
    epoch: &str,
    queue: &MessageQueue,
) -> maka_protocol::subscription::SessionMessageQueueProjection {
    use maka_protocol::subscription::*;
    let queue = project(epoch, queue);
    let message = |row: QueueEntry| QueueMessage {
        entry_id: row.entry_id,
        message_id: row.message_id,
        content: row.content,
    };
    SessionMessageQueueProjection {
        host_epoch: queue.host_epoch,
        queue_revision: queue.queue_revision,
        steering: queue
            .steering
            .into_iter()
            .map(|row| SteeringMessageSnapshot::new(message(row), SteeringState::Queued))
            .collect(),
        followup: queue
            .followup
            .into_iter()
            .map(|row| FollowupMessageSnapshot::new(message(row)))
            .collect(),
    }
}

pub(crate) fn retracted(projection: QueueProjection) -> Vec<QueueEntry> {
    projection
        .steering
        .into_iter()
        .chain(projection.followup)
        .map(|mut entry| {
            entry.state = EntryState::Retracted;
            entry
        })
        .collect()
}
