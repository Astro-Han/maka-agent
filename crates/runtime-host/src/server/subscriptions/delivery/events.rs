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

use super::super::streams::MAX_DELTA_BYTES;
use super::*;
use maka_event_log::observation::{StreamEventPage, StreamFact};
use maka_protocol::turn::TurnState;

impl Delivery {
    /// Page delivery is synchronous: only the surrounding poll owns SQL waits.
    pub(super) fn deliver_page(
        &mut self,
        page: StreamEventPage,
    ) -> Result<(Vec<Value>, bool), HostError> {
        if let Some(boundary) = page.session_boundary {
            if boundary <= self.cursor
                || boundary > page.next_after.unwrap_or(page.through_sequence)
            {
                return Err("Session transcript boundary exceeds the consumed page".into());
            }
            if let Some(transcript) = &mut self.transcript {
                transcript.catch_up_to(boundary);
            }
        }
        let mut frames = Vec::new();
        let mut events = page.events.into_iter().peekable();
        while let Some(mut stored) = events.next() {
            // Coalesce only adjacent deltas already in this bounded page. No
            // timer, cross-stream reordering, or change to canonical log facts.
            if let StreamFact::PartDelta {
                step_id,
                part_id,
                text,
            } = &mut stored.fact
            {
                while let Some(next) = events.peek() {
                    let StreamFact::PartDelta {
                        step_id: next_step,
                        part_id: next_part,
                        text: next_text,
                    } = &next.fact
                    else {
                        break;
                    };
                    if next.invocation != stored.invocation
                        || next_step != step_id
                        || next_part != part_id
                        || text.len() + next_text.len() > MAX_DELTA_BYTES
                    {
                        break;
                    }
                    text.push_str(next_text);
                    events.next();
                }
            }
            // Token observations only extend the live overlay. Wake the durable
            // transcript reader when a boundary can publish immutable rows.
            if matches!(
                stored.fact,
                StreamFact::InvocationOpened
                    | StreamFact::WorkhubDelegated
                    | StreamFact::MessageSteered
                    | StreamFact::StepEnded { .. }
                    | StreamFact::InvocationEnded { .. }
                    | StreamFact::ToolDispatched { .. }
                    | StreamFact::ToolRejected { .. }
                    | StreamFact::ToolSettled { .. }
            ) && let Some(transcript) = &mut self.transcript
            {
                transcript.catch_up_to(stored.sequence);
            }
            let pending = &self.pending.as_ref().expect("pending fence").snapshot;
            if pending.root_turn.as_ref().is_some_and(|root| {
                root.run_id == stored.root_run_id
                    && self
                        .snapshot
                        .root_turn
                        .as_ref()
                        .is_none_or(|old| old.run_id != root.run_id)
                    && matches!(
                        root.state,
                        TurnState::Admitted(_)
                            | TurnState::Created(_)
                            | TurnState::Running(_)
                            | TurnState::WaitingForUser(_)
                    )
            }) {
                // The client resets its stream accumulator when a new Run is
                // announced. Announce it before its first event, but never
                // ahead of the preceding Run's tail or before terminal text.
                self.publish(pending.clone(), &mut frames)?;
            }
            for delta in self.streams.observe(&stored)? {
                let frame = AssistantObservationFrame::SessionDelta {
                    host_epoch: self.epoch.clone(),
                    subscription_id: self.id.clone(),
                    sequence: self.sequence,
                    session_id: self.session_id.clone(),
                    delta,
                };
                let value = serde_json::to_value(frame)?;
                decode_assistant_observation_frame(&value)?;
                self.sequence += 1;
                frames.push(value);
            }
            for event in super::super::tools::events(&stored)? {
                let frame = ToolObservationFrame::SessionEvent {
                    host_epoch: self.epoch.clone(),
                    subscription_id: self.id.clone(),
                    sequence: self.sequence,
                    session_id: self.session_id.clone(),
                    run_id: stored.root_run_id.clone(),
                    event,
                };
                let value = serde_json::to_value(frame)?;
                decode_tool_observation_frame(&value)?;
                self.sequence += 1;
                frames.push(value);
            }
        }
        if let Some(after) = page.next_after {
            self.cursor = after;
            return Ok((frames, true));
        }
        let pending = self.pending.take().expect("pending fence");
        self.cursor = pending.through;
        self.publish(pending.snapshot, &mut frames)?;
        Ok((frames, false))
    }

    fn publish(
        &mut self,
        mut snapshot: SessionObservationSnapshot,
        frames: &mut Vec<Value>,
    ) -> Result<(), HostError> {
        snapshot.projection_revision = self.snapshot.projection_revision;
        if snapshot != self.snapshot {
            snapshot.projection_revision += 1;
            let frame = SessionProjectionFrame::SessionProjection {
                host_epoch: self.epoch.clone(),
                subscription_id: self.id.clone(),
                sequence: self.sequence,
                snapshot: snapshot.clone(),
            };
            frame.validate()?;
            frames.push(serde_json::to_value(frame)?);
            self.sequence += 1;
            self.snapshot = snapshot;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_event_log::observation::{StoreStreamEvent, StreamFact};
    use maka_protocol::{
        session::SessionStatus,
        turn::{LiveTurn, TurnSnapshot, TurnState},
    };
    use maka_runtime::{event::Invocation, model::TextKind};

    fn snapshot(run: &str, terminal: bool) -> SessionObservationSnapshot {
        SessionObservationSnapshot::new(
            SessionObservationIdentity {
                session_id: "s".into(),
                metadata_revision: 1,
                status: SessionStatus::Active,
                created_at: 123,
                is_archived: false,
            },
            1,
            Some(TurnSnapshot {
                session_id: "s".into(),
                turn_id: run.into(),
                run_id: run.into(),
                state: if terminal {
                    TurnState::Completed {
                        terminal_event_id: "end".into(),
                        context_compaction_outcome: None,
                    }
                } else {
                    TurnState::Running(LiveTurn::default())
                },
            }),
            None,
            SessionMessageQueueProjection {
                host_epoch: "epoch".into(),
                queue_revision: 0,
                steering: vec![],
                followup: vec![],
            },
            SessionInteractionProjection::default(),
        )
    }

    fn event(run: &str, sequence: u64, fact: StreamFact) -> StoreStreamEvent {
        StoreStreamEvent {
            root_run_id: run.into(),
            sequence,
            id: format!("event-{sequence}"),
            invocation: Invocation {
                session_id: "s".into(),
                turn_id: run.into(),
                run_id: run.into(),
                invocation_id: run.into(),
            },
            recorded_at: std::time::UNIX_EPOCH,
            fact,
        }
    }

    fn start() -> StreamFact {
        StreamFact::PartStarted {
            step_id: "step".into(),
            part_id: "text".into(),
            text_kind: TextKind::Text,
        }
    }
    fn delta(text: &str) -> StreamFact {
        StreamFact::PartDelta {
            step_id: "step".into(),
            part_id: "text".into(),
            text: text.into(),
        }
    }

    #[test]
    fn run_start_precedes_its_deltas_without_overtaking_old_tail_or_terminal_text() {
        let mut delivery = Delivery {
            version: None,
            epoch: "epoch".into(),
            id: "sub".into(),
            session_id: "s".into(),
            sequence: 1,
            cursor: 0,
            snapshot: snapshot("old", false),
            streams: Streams::default(),
            pending: Some(Pending {
                through: 40,
                snapshot: snapshot("new", false),
            }),
            transcript: None,
        };
        delivery.streams.observe(&event("old", 1, start())).unwrap();
        let finish = || StreamFact::StepEnded {
            step_id: "step".into(),
            failed: false,
            interrupted: vec![],
        };
        // The first catch-up page is still the preceding Run. Its tail must
        // finish before the new Run can reset the unchanged client's projector.
        let (mut frames, more) = delivery
            .deliver_page(StreamEventPage {
                session_boundary: None,
                events: vec![
                    event("old", 2, delta("old tail")),
                    event("old", 3, finish()),
                ],
                through_sequence: 40,
                next_after: Some(3),
            })
            .unwrap();
        assert!(more);
        assert!(
            frames
                .iter()
                .all(|frame| frame["kind"] == "subscription.session_delta")
        );
        let (new, more) = delivery
            .deliver_page(StreamEventPage {
                session_boundary: None,
                events: vec![event("new", 4, start()), event("new", 5, delta("hello"))],
                through_sequence: 40,
                next_after: Some(5),
            })
            .unwrap();
        assert!(more);
        assert_eq!(new[0]["kind"], "subscription.session_projection");
        assert_eq!(new[0]["snapshot"]["rootTurn"]["runId"], "new");
        assert_eq!(new[1]["delta"]["startOffset"], 0);
        frames.extend(new);
        // The same pending fence spans another page and must not announce the
        // Run a second time, which would clear the partially assembled text.
        let (new, more) = delivery
            .deliver_page(StreamEventPage {
                session_boundary: None,
                events: vec![
                    event("new", 6, delta(" 🌍")),
                    event("new", 7, delta(" world")),
                ],
                through_sequence: 40,
                next_after: None,
            })
            .unwrap();
        assert!(!more);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0]["delta"]["startOffset"], 5);
        assert_eq!(new[0]["delta"]["text"], " 🌍 world");
        frames.extend(new);
        delivery.pending = Some(Pending {
            through: 50,
            snapshot: snapshot("new", true),
        });
        let (terminal, more) = delivery
            .deliver_page(StreamEventPage {
                session_boundary: None,
                events: vec![event("new", 41, delta("!")), event("new", 42, finish())],
                through_sequence: 50,
                next_after: None,
            })
            .unwrap();
        assert!(!more);
        assert_eq!(terminal[0]["delta"]["startOffset"], 14);
        assert_eq!(terminal[1]["delta"]["complete"], true);
        assert_eq!(terminal[2]["snapshot"]["rootTurn"]["status"], "completed");
        assert_eq!(terminal[2]["snapshot"]["projectionRevision"], 3);
        frames.extend(terminal);
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame["sequence"], (index + 1) as u64);
        }
    }
}
