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

use super::HostError;
use maka_protocol::Response;
mod pty;
mod scheduling;
use maka_transport::MessageWriter;
use scheduling::{Lane, lane};
use serde_json::Value;
use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;
use tokio_util::{sync::CancellationToken, task::task_tracker::TaskTrackerToken};

const MAX_FRAMES: usize = 64;
const MAX_BYTES: usize = 2 * 1024 * 1024;

struct Frame {
    value: Value,
    lane: Lane,
    bytes: usize,
    fatal: bool,
    // Kept until this exact frame flushes or the transport is abandoned.
    _residency: Option<TaskTrackerToken>,
}
#[derive(Default)]
struct State {
    queue: VecDeque<Frame>,
    bytes: usize,
    frames: usize,
    closed: bool,
    control_burst: u8,
    data_lane: usize,
    flushed: VecDeque<String>,
    pty_pending: HashSet<String>,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Notify,
    flushed: Notify,
    pty_flushed: Notify,
}

/// One connection-owned bounded queue. Enqueue never waits behind native output.
#[derive(Default)]
pub(super) struct Outbound(Arc<Shared>);

impl Outbound {
    pub fn try_flushed(&self) -> Option<String> {
        self.0.state.lock().unwrap().flushed.pop_front()
    }
    pub async fn flushed(&self) -> String {
        loop {
            let notified = self.0.flushed.notified();
            if let Some(id) = self.try_flushed() {
                return id;
            }
            notified.await;
        }
    }
    pub async fn enqueue(&self, value: Value) -> Result<(), HostError> {
        self.push(value, None, false).await
    }
    pub async fn reply(
        &self,
        response: Response,
        residency: Option<TaskTrackerToken>,
        fatal: bool,
    ) -> Result<(), HostError> {
        self.push(serde_json::to_value(response)?, residency, fatal)
            .await
    }
    async fn push(
        &self,
        value: Value,
        residency: Option<TaskTrackerToken>,
        fatal: bool,
    ) -> Result<(), HostError> {
        let bytes = serde_json::to_vec(&value)?.len() + 1;
        let lane = lane(&value);
        {
            let mut state = self.0.state.lock().unwrap();
            if state.closed {
                return Err("connection outbound closed".into());
            }
            if state.frames == MAX_FRAMES || bytes > MAX_BYTES.saturating_sub(state.bytes) {
                return Err("connection outbound capacity exhausted".into());
            }
            if lane == Lane::Pty
                && !state.pty_pending.insert(
                    value["subscriptionId"]
                        .as_str()
                        .ok_or("missing PTY subscription")?
                        .to_owned(),
                )
            {
                return Err("PTY cursor advanced before its prior frame flushed".into());
            }
            state.bytes += bytes;
            state.frames += 1;
            state.queue.push_back(Frame {
                value,
                lane,
                bytes,
                fatal,
                _residency: residency,
            });
        }
        self.0.changed.notify_one();
        // Cooperate with the joined writer even when every producer is ready.
        // This yields scheduling, never waits for socket capacity or a flush.
        // Actual slow-client saturation retains the protocol's fail-closed bound.
        tokio::task::yield_now().await;
        Ok(())
    }
    pub fn close(&self) {
        self.0.state.lock().unwrap().closed = true;
        self.0.changed.notify_one();
    }

    pub async fn run(
        &self,
        mut writer: impl MessageWriter,
        closed: &CancellationToken,
    ) -> Result<(), HostError> {
        let result = async {
            loop {
                let changed = self.0.changed.notified();
                let (frame, finished) = {
                    let mut state = self.0.state.lock().unwrap();
                    let frame = state.pop();
                    let finished = frame.is_none() && state.closed;
                    (frame, finished)
                };
                let Some(frame) = frame else {
                    if finished {
                        tokio::time::timeout(Duration::from_secs(2), writer.close_after_flush()).await??;
                        return Ok(());
                    }
                    tokio::select! {
                        _ = closed.cancelled() => return Ok(()),
                        _ = changed => {},
                    }
                    continue;
                };
                // This future is polled to completion once, never recreated on a read wakeup.
                tokio::select! {
                    biased;
                    _ = closed.cancelled() => return Ok(()),
                    result = tokio::time::timeout(Duration::from_secs(5), writer.write(&frame.value)) => { result??; }
                }
                let fatal = frame.fatal;
                {
                    let mut state = self.0.state.lock().unwrap();
                    state.bytes -= frame.bytes;
                    state.frames -= 1;
                    if frame.lane == Lane::Pty {
                        state.pty_pending.remove(frame.value["subscriptionId"].as_str().expect("validated PTY subscription"));
                        self.0.pty_flushed.notify_one();
                    }
                    if let Some(id) = frame.value.get("requestId").and_then(Value::as_str) {
                        state.flushed.push_back(id.to_owned());
                        self.0.flushed.notify_one();
                    }
                }
                drop(frame);
                if fatal { return Ok(()); }
            }
        }.await;
        closed.cancel();
        let mut state = self.0.state.lock().unwrap();
        state.closed = true;
        state.queue.clear();
        state.bytes = 0;
        state.frames = 0;
        state.pty_pending.clear();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Writer(Arc<Mutex<Vec<Value>>>);
    impl MessageWriter for Writer {
        async fn write(&mut self, value: &Value) -> maka_transport::Result<()> {
            self.0.lock().unwrap().push(value.clone());
            Ok(())
        }
        async fn close_after_flush(&mut self) -> maka_transport::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn joined_writer_progresses_through_a_batch_larger_than_its_queue() {
        let outbound = Outbound::default();
        let received = Arc::new(Mutex::new(Vec::new()));
        let closed = CancellationToken::new();
        let publish = async {
            for sequence in 0..256 {
                outbound
                    .enqueue(json!({"kind":"subscription.delta", "sequence":sequence}))
                    .await
                    .unwrap();
            }
            outbound
                .enqueue(json!({"kind":"subscription.closed"}))
                .await
                .unwrap();
            outbound.close();
        };
        let (_, delivery) = tokio::join!(publish, outbound.run(Writer(received.clone()), &closed));
        delivery.unwrap();
        let received = received.lock().unwrap();
        assert_eq!(received.len(), 257);
        for (sequence, frame) in received[..256].iter().enumerate() {
            assert_eq!(frame["sequence"], sequence);
        }
        assert_eq!(received[256]["kind"], "subscription.closed");
    }
}
