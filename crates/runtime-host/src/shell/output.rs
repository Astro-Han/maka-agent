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

use super::{Result, ShellError};
use maka_runtime::terminal::TerminalSize;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

const REPLAY_BYTES: usize = 80 * 1024;
const FRAME_BYTES: usize = 16 * 1024;
const FRAME_COUNT: usize = 8;
const MAX_SEQUENCE: u64 = 9_007_199_254_740_991;

/// An atomic, ephemeral terminal cut, not a durable resource revision.
#[derive(Clone, Debug)]
pub struct PtyReplay {
    pub sequence: u64,
    pub buffer: String,
    pub size: TerminalSize,
}

#[derive(Debug)]
pub struct PtyData {
    pub sequence: u64,
    pub data: String,
}

#[derive(Debug)]
pub enum PtyStreamEvent {
    Data(Arc<PtyData>),
    /// The cursor fell behind the bounded ring. Replace the prior terminal cut.
    Reset(PtyReplay),
    Closed,
}

struct State {
    replay: PtyReplay,
    frames: VecDeque<Arc<PtyData>>,
    closed: bool,
}

#[derive(Clone)]
pub(super) struct Output(Arc<Shared>);
struct Shared {
    state: Mutex<State>,
    changed: Notify,
    host_changes: tokio::sync::watch::Sender<()>,
}

/// Independent observation cursor. Dropping it never cancels its process.
pub struct PtyStream {
    output: Output,
    sequence: u64,
}

impl Output {
    pub fn new(size: TerminalSize, host_changes: tokio::sync::watch::Sender<()>) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                replay: PtyReplay {
                    sequence: 0,
                    buffer: String::new(),
                    size,
                },
                frames: VecDeque::new(),
                closed: false,
            }),
            changed: Notify::new(),
            host_changes,
        }))
    }

    pub fn replay(&self) -> PtyReplay {
        self.0.state.lock().unwrap().replay.clone()
    }

    pub fn stream(&self) -> PtyStream {
        PtyStream {
            output: self.clone(),
            sequence: self.0.state.lock().unwrap().replay.sequence,
        }
    }

    pub fn attach(&self) -> (PtyReplay, PtyStream) {
        let replay = self.replay();
        let sequence = replay.sequence;
        (
            replay,
            PtyStream {
                output: self.clone(),
                sequence,
            },
        )
    }

    /// One parser cut is published atomically. Sequence increments per emitted
    /// frame, not per native read or pre-coalescing chunk.
    pub fn publish(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let chunks = chunks(text);
        let mut state = self.0.state.lock().unwrap();
        if state.closed || state.replay.sequence > MAX_SEQUENCE - chunks.len() as u64 {
            return Err(ShellError::Rejected(
                "PTY output sequence exhausted or closed",
            ));
        }
        state.replay.buffer.push_str(text);
        let excess = state.replay.buffer.len().saturating_sub(REPLAY_BYTES);
        let cut = state.replay.buffer.ceil_char_boundary(excess);
        state.replay.buffer.drain(..cut);
        for data in chunks {
            state.replay.sequence += 1;
            let sequence = state.replay.sequence;
            if state.frames.len() == FRAME_COUNT {
                state.frames.pop_front();
            }
            state.frames.push_back(Arc::new(PtyData {
                sequence,
                data: data.into(),
            }));
        }
        drop(state);
        self.0.changed.notify_waiters();
        self.0.host_changes.send_replace(());
        Ok(())
    }

    pub fn resize(&self, size: TerminalSize) {
        self.0.state.lock().unwrap().replay.size = size;
    }

    pub fn close(&self) {
        self.0.state.lock().unwrap().closed = true;
        self.0.changed.notify_waiters();
        self.0.host_changes.send_replace(());
    }
}

impl PtyStream {
    pub(crate) fn try_next(&mut self) -> Option<PtyStreamEvent> {
        let state = self.output.0.state.lock().unwrap();
        if self.sequence < state.replay.sequence {
            if let Some(frame) = state
                .frames
                .iter()
                .find(|f| f.sequence == self.sequence + 1)
            {
                self.sequence = frame.sequence;
                return Some(PtyStreamEvent::Data(frame.clone()));
            }
            self.sequence = state.replay.sequence;
            return Some(PtyStreamEvent::Reset(state.replay.clone()));
        }
        state.closed.then_some(PtyStreamEvent::Closed)
    }

    pub async fn next(&mut self) -> PtyStreamEvent {
        let output = self.output.clone();
        loop {
            // Register before inspecting the cut: no lost wakeup at attach/EOF.
            let changed = output.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(event) = self.try_next() {
                return event;
            }
            changed.await;
        }
    }
}

/// Bound JSON-escaped data, not merely UTF-8: C0 output can expand sixfold.
/// Frame metadata has its own protocol budget. Eight frames retain <=128 KiB.
fn chunks(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let (mut start, mut bytes) = (0, 0);
    for (offset, ch) in text.char_indices() {
        let cost = match ch {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
            '\u{00}'..='\u{1f}' => 6,
            _ => ch.len_utf8(),
        };
        if bytes + cost > FRAME_BYTES {
            chunks.push(&text[start..offset]);
            start = offset;
            bytes = 0;
        }
        bytes += cost;
    }
    if start != text.len() {
        chunks.push(&text[start..]);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn attachment_cuts_lag_and_eof_preserve_unicode_and_bounded_json_frames() {
        let output = Output::new(
            TerminalSize::new(80, 24).unwrap(),
            tokio::sync::watch::channel(()).0,
        );
        output.publish("before中").unwrap();
        let (cut, mut stream) = output.attach();
        assert_eq!(cut.buffer, "before中");
        let data = "\0中😀\x1b\\\"".repeat(1800);
        output.publish(&data).unwrap();
        let last = output.replay().sequence;
        let mut actual = String::new();
        for sequence in cut.sequence + 1..=last {
            let PtyStreamEvent::Data(frame) = stream.next().await else {
                panic!("unexpected gap")
            };
            assert_eq!(frame.sequence, sequence);
            assert!(serde_json::to_vec(&frame.data).unwrap().len() <= FRAME_BYTES + 2);
            actual.push_str(&frame.data);
        }
        assert_eq!(actual, data);
        let (_, mut lagged) = output.attach();
        for _ in 0..20 {
            output.publish(&"中".repeat(4000)).unwrap();
        }
        let PtyStreamEvent::Reset(cut) = lagged.next().await else {
            panic!("missing reset")
        };
        assert!(cut.buffer.len() <= REPLAY_BYTES);
        assert!(cut.buffer.chars().all(|ch| ch == '中'));
        let pending = lagged.next();
        tokio::pin!(pending);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), &mut pending)
                .await
                .is_err()
        );
        output.close();
        assert!(matches!(pending.await, PtyStreamEvent::Closed));
    }
}
