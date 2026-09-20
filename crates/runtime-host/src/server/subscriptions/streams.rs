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

use super::super::HostError;
use maka_event_log::observation::{AssistantStreamSeed, StoreStreamEvent, StreamFact};
use maka_protocol::subscription::{
    AssistantStreamKind, SessionAssistantDelta, SessionAssistantStreamIdentity, TrueFlag,
};
use maka_runtime::event::Invocation;
use maka_runtime::model::TextKind;
use std::collections::BTreeMap;

pub(super) const MAX_DELTA_BYTES: usize = 8 * 1024;

struct ActiveStream {
    turn_id: String,
    run_id: String,
    message_id: String,
    kind: AssistantStreamKind,
    offset: u64,
}

/// Disposable projection of committed stream facts. No execution ownership.
#[derive(Default)]
pub(super) struct Streams(BTreeMap<(String, String), ActiveStream>);

impl Streams {
    pub fn bootstrap(invocation: Option<&Invocation>, seeds: Vec<AssistantStreamSeed>) -> Self {
        let mut streams = Self::default();
        if let Some(invocation) = invocation {
            for seed in seeds {
                streams.0.insert(
                    (seed.step_id, seed.part_id),
                    ActiveStream {
                        turn_id: invocation.turn_id.clone(),
                        run_id: invocation.run_id.clone(),
                        message_id: seed.message_id,
                        kind: kind(seed.text_kind),
                        offset: 0,
                    },
                );
            }
        }
        streams
    }

    pub fn identities(&self) -> Vec<SessionAssistantStreamIdentity> {
        self.0
            .values()
            .map(|stream| SessionAssistantStreamIdentity {
                kind: stream.kind,
                turn_id: stream.turn_id.clone(),
                message_id: stream.message_id.clone(),
            })
            .collect()
    }

    pub fn observe(
        &mut self,
        event: &StoreStreamEvent,
    ) -> Result<Vec<SessionAssistantDelta>, HostError> {
        let mut deltas = Vec::new();
        match &event.fact {
            StreamFact::ExecutorStarted => {
                for text_kind in [TextKind::Text, TextKind::Thinking] {
                    self.start(
                        event,
                        event.invocation.invocation_id.clone(),
                        executor_part(text_kind).into(),
                        text_kind,
                    )?;
                }
            }
            StreamFact::ExecutorDelta { text_kind, text } => {
                let stream = self
                    .0
                    .get_mut(&(
                        event.invocation.invocation_id.clone(),
                        executor_part(*text_kind).into(),
                    ))
                    .ok_or("committed executor delta has no assistant stream")?;
                stream.append(text, &mut deltas);
            }
            StreamFact::ExecutorCompleted => {
                self.finish_matching(
                    |(invocation, _)| invocation == &event.invocation.invocation_id,
                    &[],
                    &mut deltas,
                );
            }
            StreamFact::PartStarted {
                step_id,
                part_id: id,
                text_kind,
            } => {
                self.start(event, step_id.clone(), id.clone(), *text_kind)?;
            }
            StreamFact::PartDelta {
                step_id,
                part_id: id,
                text,
            } => {
                let stream = self
                    .0
                    .get_mut(&(step_id.clone(), id.clone()))
                    .ok_or("committed delta has no assistant stream")?;
                stream.append(text, &mut deltas);
            }
            StreamFact::PartFinished {
                step_id,
                part_id: id,
            } => {
                // A part can finish before its request fails. Keep its identity
                // until the request verdict, including across subscriber reconnects.
                if !self.0.contains_key(&(step_id.clone(), id.clone())) {
                    return Err("committed completion has no assistant stream".into());
                }
            }
            StreamFact::StepEnded {
                step_id,
                interrupted,
                ..
            } => {
                self.finish_matching(|(step, _)| step == step_id, interrupted, &mut deltas);
            }
            StreamFact::InvocationEnded { interrupted, .. } => {
                self.finish_matching(|_| true, interrupted, &mut deltas);
            }
            StreamFact::InvocationOpened
            | StreamFact::MessageSteered
            | StreamFact::ToolDispatched { .. }
            | StreamFact::ToolRejected { .. }
            | StreamFact::ToolSettled { .. }
            | StreamFact::ExecutorToolStart { .. }
            | StreamFact::ExecutorToolProgress { .. }
            | StreamFact::ExecutorToolResult { .. } => {}
        }
        Ok(deltas)
    }

    fn start(
        &mut self,
        event: &StoreStreamEvent,
        step: String,
        part: String,
        text_kind: TextKind,
    ) -> Result<(), HostError> {
        if self.0.len() >= 128 {
            return Err("too many simultaneous assistant streams".into());
        }
        if self.0.contains_key(&(step.clone(), part.clone())) {
            return Err("duplicate committed assistant stream".into());
        }
        self.0.insert(
            (step, part),
            ActiveStream {
                turn_id: event.invocation.turn_id.clone(),
                run_id: event.root_run_id.clone(),
                message_id: event.id.clone(),
                kind: kind(text_kind),
                offset: 0,
            },
        );
        Ok(())
    }

    fn finish_matching(
        &mut self,
        matches: impl Fn(&(String, String)) -> bool,
        interrupted: &[String],
        deltas: &mut Vec<SessionAssistantDelta>,
    ) {
        let text_messages: std::collections::BTreeSet<_> = self
            .0
            .iter()
            .filter(|(key, stream)| matches(key) && stream.kind == AssistantStreamKind::Text)
            .map(|(_, stream)| stream.message_id.clone())
            .collect();
        self.0.retain(|key, stream| {
            if matches(key) {
                let interrupted = interrupted.contains(&stream.message_id);
                deltas.push(stream.delta(String::new(), true, interrupted));
                if interrupted
                    && stream.kind == AssistantStreamKind::Thinking
                    && !text_messages.contains(&stream.message_id)
                {
                    // The existing client renders the interruption divider from text,
                    // even when the provider produced only reasoning.
                    let mut divider = stream.delta(String::new(), true, true);
                    divider.kind = AssistantStreamKind::Text;
                    divider.start_offset = 0;
                    deltas.push(divider);
                }
                false
            } else {
                true
            }
        });
    }
}

impl ActiveStream {
    fn append(&mut self, mut remaining: &str, deltas: &mut Vec<SessionAssistantDelta>) {
        while !remaining.is_empty() {
            let mut end = remaining.len().min(MAX_DELTA_BYTES);
            while !remaining.is_char_boundary(end) {
                end -= 1;
            }
            let (text, rest) = remaining.split_at(end);
            deltas.push(self.delta(text.to_owned(), false, false));
            remaining = rest;
        }
    }
    fn delta(&mut self, text: String, complete: bool, interrupted: bool) -> SessionAssistantDelta {
        let start_offset = self.offset;
        self.offset += text.encode_utf16().count() as u64;
        SessionAssistantDelta {
            kind: self.kind,
            turn_id: self.turn_id.clone(),
            run_id: self.run_id.clone(),
            message_id: self.message_id.clone(),
            start_offset,
            text,
            reset: None,
            complete: complete.then_some(TrueFlag),
            interrupted: interrupted.then_some(TrueFlag),
        }
    }
}

fn executor_part(kind: TextKind) -> &'static str {
    match kind {
        TextKind::Text => "text",
        TextKind::Thinking => "thinking",
    }
}

fn kind(kind: TextKind) -> AssistantStreamKind {
    match kind {
        TextKind::Text => AssistantStreamKind::Text,
        TextKind::Thinking => AssistantStreamKind::Thinking,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_streams_share_durable_identity_and_preserve_utf16_offsets_after_replay() {
        let invocation = Invocation {
            session_id: "s".into(),
            turn_id: "t".into(),
            run_id: "r".into(),
            invocation_id: "i".into(),
        };
        let event = |fact| StoreStreamEvent {
            root_run_id: "r".into(),
            sequence: 1,
            id: "external-message".into(),
            invocation: invocation.clone(),
            recorded_at: std::time::UNIX_EPOCH,
            fact,
        };
        // Both fresh delivery and reconnect replay start at the committed start.
        for interrupted in [false, true] {
            let mut streams = Streams::default();
            streams
                .observe(&event(StreamFact::ExecutorStarted))
                .unwrap();
            for text_kind in [TextKind::Text, TextKind::Thinking] {
                let text = "🌍".repeat(MAX_DELTA_BYTES);
                let deltas = streams
                    .observe(&event(StreamFact::ExecutorDelta {
                        text_kind,
                        text: text.clone(),
                    }))
                    .unwrap();
                assert_eq!(
                    deltas
                        .iter()
                        .map(|delta| delta.text.as_str())
                        .collect::<String>(),
                    text
                );
                for (index, delta) in deltas.iter().enumerate() {
                    assert_eq!(delta.message_id, "external-message");
                    assert_eq!(delta.kind, kind(text_kind));
                    assert_eq!(delta.start_offset, (index * MAX_DELTA_BYTES / 2) as u64);
                    assert!(delta.text.len() <= MAX_DELTA_BYTES);
                }
            }
            let deltas = streams
                .observe(&event(if interrupted {
                    StreamFact::InvocationEnded {
                        failed: true,
                        interrupted: vec!["external-message".into()],
                    }
                } else {
                    StreamFact::ExecutorCompleted
                }))
                .unwrap();
            assert_eq!(deltas.len(), 2);
            assert!(deltas.iter().all(
                |delta| delta.complete.is_some() && delta.interrupted.is_some() == interrupted
            ));
            assert!(streams.identities().is_empty());
        }
    }

    #[test]
    fn closed_parts_reconnect_until_verdict_and_only_failed_fragments_get_dividers() {
        let invocation = Invocation {
            session_id: "session".into(),
            turn_id: "turn".into(),
            run_id: "run".into(),
            invocation_id: "invocation".into(),
        };
        let root = Invocation {
            run_id: "root".into(),
            invocation_id: "original-invocation".into(),
            ..invocation.clone()
        };
        for failed in [false, true] {
            let seeds: Vec<_> = [
                ("text", TextKind::Text),
                ("unfinished", TextKind::Thinking),
                ("finalized", TextKind::Thinking),
            ]
            .into_iter()
            .map(|(id, text_kind)| AssistantStreamSeed {
                start_sequence: 1,
                step_id: "step".into(),
                part_id: id.into(),
                message_id: id.into(),
                text_kind,
            })
            .collect();
            let event = |fact| StoreStreamEvent {
                root_run_id: root.run_id.clone(),
                sequence: 10,
                id: "boundary".into(),
                invocation: invocation.clone(),
                recorded_at: std::time::SystemTime::UNIX_EPOCH,
                fact,
            };
            let mut streams = {
                let mut streams = Streams::default();
                for seed in seeds {
                    let mut start = event(StreamFact::PartStarted {
                        step_id: seed.step_id.clone(),
                        part_id: seed.part_id.clone(),
                        text_kind: seed.text_kind,
                    });
                    start.id = seed.message_id;
                    streams.observe(&start).unwrap();
                    let deltas = streams
                        .observe(&event(StreamFact::PartDelta {
                            step_id: seed.step_id,
                            part_id: seed.part_id,
                            text: "12345".into(),
                        }))
                        .unwrap();
                    assert_eq!(deltas[0].run_id, root.run_id);
                }
                streams
            };
            assert!(
                streams
                    .observe(&event(StreamFact::PartFinished {
                        step_id: "step".into(),
                        part_id: "text".into(),
                    }))
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(streams.identities().len(), 3);
            let deltas = streams
                .observe(&event(StreamFact::StepEnded {
                    step_id: "step".into(),
                    failed,
                    interrupted: if failed {
                        vec!["text".into(), "unfinished".into()]
                    } else {
                        vec![]
                    },
                }))
                .unwrap();
            assert_eq!(deltas.len(), if failed { 4 } else { 3 });
            for delta in &deltas {
                assert_eq!(
                    delta.run_id, root.run_id,
                    "fresh and reconnected streams keep the logical identity"
                );
                assert!(delta.complete.is_some());
                assert!(delta.text.is_empty());
                assert_eq!(
                    delta.interrupted.is_some(),
                    failed && delta.message_id != "finalized"
                );
                let divider =
                    delta.message_id == "unfinished" && delta.kind == AssistantStreamKind::Text;
                assert_eq!(delta.start_offset, if divider { 0 } else { 5 });
            }
            assert!(streams.identities().is_empty());
            assert!(
                streams
                    .observe(&event(StreamFact::InvocationEnded {
                        failed,
                        interrupted: vec![]
                    }))
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
