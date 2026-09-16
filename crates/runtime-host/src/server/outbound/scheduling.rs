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

use super::{Frame, State};
use maka_protocol::Operation;
use serde_json::Value;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Lane {
    Control,
    State,
    Bulk,
    Pty,
    Barrier,
}

impl State {
    pub(super) fn pop(&mut self) -> Option<Frame> {
        let end = self
            .queue
            .iter()
            .position(|frame| frame.lane == Lane::Barrier)
            .unwrap_or(self.queue.len());
        if end == 0 {
            self.control_burst = 0;
            return self.queue.pop_front();
        }
        let control = self
            .queue
            .iter()
            .take(end)
            .position(|frame| frame.lane == Lane::Control);
        if self.control_burst < 8
            && let Some(index) = control
        {
            self.control_burst += 1;
            return self.queue.remove(index);
        }
        let lanes = [Lane::State, Lane::Bulk, Lane::Pty];
        for offset in 0..lanes.len() {
            let current = (self.data_lane + offset) % lanes.len();
            if let Some(index) = self
                .queue
                .iter()
                .take(end)
                .position(|frame| frame.lane == lanes[current])
            {
                self.data_lane = (current + 1) % lanes.len();
                self.control_burst = 0;
                return self.queue.remove(index);
            }
        }
        self.control_burst = self.control_burst.saturating_add(1);
        control.and_then(|index| self.queue.remove(index))
    }
}

pub(super) fn lane(value: &Value) -> Lane {
    if let Some(operation) = value
        .get("operation")
        .and_then(Value::as_str)
        .and_then(|name| name.parse::<Operation>().ok())
    {
        return match operation {
            // No stream data may precede open or follow its close acknowledgement.
            Operation::SubscriptionOpen
            | Operation::SubscriptionReady
            | Operation::SubscriptionClose => Lane::Barrier,
            Operation::SessionTranscriptPage
            | Operation::ArtifactQuery
            | Operation::RuntimeResourceQuery => Lane::Bulk,
            _ => Lane::Control,
        };
    }
    match value.get("kind").and_then(Value::as_str) {
        Some("subscription.closed") => Lane::Barrier,
        Some("subscription.runtime_resource_pty_data") => Lane::Pty,
        Some(kind) if kind.starts_with("subscription.") => Lane::State,
        _ => Lane::Control,
    }
}
