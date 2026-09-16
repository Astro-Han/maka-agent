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

use super::super::{Host, HostError, outbound::Outbound};
use crate::shell::{PtyStream, PtyStreamEvent};
use maka_presentation::shell::RESOURCE_REF_PREFIX;
use maka_protocol::subscription::{
    ResourceObservationFrame, TrueFlag, decode_resource_observation_frame,
};
use serde_json::Value;

struct Interest {
    subscription: String,
    session: String,
    reference: String,
    stream: Option<PtyStream>,
}

/// Cursors, not copied output queues. One queued/write-in-flight frame per
/// subscription bounds delivery independently of native PTY production.
#[derive(Default)]
pub(super) struct PtyObservers {
    interests: Vec<Interest>,
    next: usize,
}

impl PtyObservers {
    pub fn set(&mut self, host: &Host, subscription: &str, session: &str, refs: &[String]) {
        self.interests
            .retain(|entry| entry.subscription != subscription || refs.contains(&entry.reference));
        for reference in refs {
            if self
                .interests
                .iter()
                .any(|entry| entry.subscription == subscription && entry.reference == *reference)
            {
                continue;
            }
            self.interests.push(Interest {
                subscription: subscription.into(),
                session: session.into(),
                reference: reference.clone(),
                stream: attach(host, session, reference),
            });
        }
    }

    pub fn remove(&mut self, subscription: &str) {
        self.interests
            .retain(|entry| entry.subscription != subscription);
    }

    pub fn refresh(&mut self, host: &Host, change: &maka_event_log::shell_runs::ShellChange) {
        for entry in &mut self.interests {
            if entry.stream.is_none()
                && entry.session == change.session_id
                && entry.reference.strip_prefix(RESOURCE_REF_PREFIX) == Some(change.id.as_str())
            {
                entry.stream = attach(host, &entry.session, &entry.reference);
            }
        }
    }

    pub fn poll(
        &mut self,
        host: &Host,
        outbound: &Outbound,
        ready: impl Fn(&str) -> bool,
    ) -> Result<Option<Value>, HostError> {
        let count = self.interests.len();
        for offset in 0..count {
            let index = (self.next + offset) % count;
            let entry = &mut self.interests[index];
            if !ready(&entry.subscription) || !outbound.pty_ready(&entry.subscription) {
                continue;
            }
            let Some(event) = entry.stream.as_mut().and_then(PtyStream::try_next) else {
                continue;
            };
            let (sequence, data, reset) = match event {
                PtyStreamEvent::Data(frame) => (frame.sequence, frame.data.clone(), None),
                PtyStreamEvent::Reset(cut) => (cut.sequence, String::new(), Some(TrueFlag)),
                PtyStreamEvent::Closed => {
                    entry.stream = None;
                    continue;
                }
            };
            self.next = index + 1;
            let frame = ResourceObservationFrame::PtyData {
                host_epoch: host.epoch.clone(),
                subscription_id: entry.subscription.clone(),
                session_id: entry.session.clone(),
                resource_ref: entry.reference.clone(),
                pty_sequence: sequence,
                data,
                reset,
            };
            let value = serde_json::to_value(frame)?;
            decode_resource_observation_frame(&value)?;
            return Ok(Some(value));
        }
        Ok(None)
    }
}

fn attach(host: &Host, session: &str, reference: &str) -> Option<PtyStream> {
    let id = reference.strip_prefix(RESOURCE_REF_PREFIX)?;
    host.shells.get(session, id)?.stream()
}
