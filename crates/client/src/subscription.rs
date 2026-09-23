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

use crate::{Client, ClientError, RequestFailure};
use maka_protocol::{Operation, ProtocolError, Result, subscription::*, transcript::*};
use serde_json::Value;
use std::collections::HashMap;

impl Client {
    /// Install the returned snapshot in the consumer before calling ready.
    pub async fn open_subscription(
        &self,
        input: SubscriptionOpenInput,
    ) -> std::result::Result<SubscriptionOpenResult, RequestFailure> {
        let value = self
            .request(
                Operation::SubscriptionOpen,
                serde_json::to_value(input).expect("wire input"),
            )
            .await?;
        decode_subscription_open_result(&value).map_err(|error| self.invalid_observation(error))
    }
    pub async fn ready_subscription(&self, id: &str) -> std::result::Result<(), RequestFailure> {
        self.request(
            Operation::SubscriptionReady,
            serde_json::json!({"subscriptionId": id}),
        )
        .await?;
        Ok(())
    }
    pub async fn close_subscription(&self, id: &str) -> std::result::Result<(), RequestFailure> {
        self.request(
            Operation::SubscriptionClose,
            serde_json::json!({"subscriptionId": id}),
        )
        .await?;
        Ok(())
    }
    pub async fn transcript_page(
        &self,
        input: SessionTranscriptPageInput,
    ) -> std::result::Result<SessionTranscriptPage, RequestFailure> {
        let value = self
            .request(
                Operation::SessionTranscriptPage,
                serde_json::to_value(input).expect("wire input"),
            )
            .await?;
        decode_session_transcript_page(&value).map_err(|error| self.invalid_observation(error))
    }
    pub async fn transcript_search(
        &self,
        input: TranscriptSearchInput,
    ) -> std::result::Result<TranscriptSearchResult, RequestFailure> {
        let value = self
            .request(
                Operation::SessionTranscriptSearch,
                serde_json::to_value(input).expect("wire input"),
            )
            .await?;
        decode_transcript_search_result(&value).map_err(|error| self.invalid_observation(error))
    }
    fn invalid_observation(&self, error: ProtocolError) -> RequestFailure {
        self.disconnect();
        RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
    }
}

/// Request correlation survives a page close or a caller timeout.
pub(crate) enum PendingObservation {
    None,
    Open(SubscriptionOpenInput),
    Ack {
        id: String,
        close: bool,
    },
    Page {
        input: SessionTranscriptPageInput,
        session: String,
    },
    Search {
        input: TranscriptSearchInput,
        session: String,
    },
}
impl PendingObservation {
    pub fn opens_subscription(&self) -> bool {
        matches!(self, Self::Open(_))
    }
}

struct Observation {
    session: String,
    next_sequence: u64,
    projection_revision: u64,
    through_sequence: Option<u64>,
    ready: bool,
}

/// Owned solely by the connection reader, never concurrently mutated by UI jobs.
#[derive(Default)]
pub(crate) struct Subscriptions(HashMap<String, Observation>);

impl Subscriptions {
    pub fn prepare(&mut self, operation: Operation, value: &Value) -> Result<PendingObservation> {
        Ok(match operation {
            Operation::SubscriptionOpen => {
                PendingObservation::Open(decode_subscription_open_input(value)?)
            }
            Operation::SubscriptionClose => PendingObservation::Ack {
                id: decode_subscription_close_input(value)?.subscription_id,
                close: true,
            },
            Operation::SubscriptionReady | Operation::SubscriptionPtyInterestSet => {
                let id = if operation == Operation::SubscriptionReady {
                    decode_subscription_close_input(value)?.subscription_id
                } else {
                    decode_pty_interest_input(value)?.subscription_id
                };
                let state = self
                    .0
                    .get_mut(&id)
                    .ok_or_else(|| invalid("Unknown subscription"))?;
                if operation == Operation::SubscriptionReady {
                    // Host frames can precede the ready acknowledgement.
                    state.ready = true;
                }
                PendingObservation::Ack { id, close: false }
            }
            Operation::SessionTranscriptPage => {
                let input = decode_session_transcript_page_input(value)?;
                let state = self
                    .0
                    .get(&input.subscription_id)
                    .ok_or_else(|| invalid("Unknown subscription"))?;
                PendingObservation::Page {
                    input,
                    session: state.session.clone(),
                }
            }
            Operation::SessionTranscriptSearch => {
                let input = decode_transcript_search_input(value)?;
                let state = self
                    .0
                    .get(&input.subscription_id)
                    .ok_or_else(|| invalid("Unknown subscription"))?;
                PendingObservation::Search {
                    input,
                    session: state.session.clone(),
                }
            }
            _ => PendingObservation::None,
        })
    }

    pub fn complete(
        &mut self,
        pending: &PendingObservation,
        value: &Value,
        epoch: &str,
    ) -> Result<()> {
        match pending {
            PendingObservation::Search { input, session } => {
                validate_search_result(input, &decode_transcript_search_result(value)?, session)?;
            }
            PendingObservation::None => {}
            PendingObservation::Open(input) => {
                let output = decode_subscription_open_result(value)?;
                output.validate_for(input, epoch)?;
                if self.0.len() >= 16 || self.0.contains_key(&output.subscription_id) {
                    return Err(invalid(
                        "Duplicate subscription or subscription limit exceeded",
                    ));
                }
                self.0.insert(
                    output.subscription_id,
                    Observation {
                        session: output.snapshot.session.session_id,
                        next_sequence: output.next_sequence,
                        projection_revision: output.snapshot.projection_revision,
                        through_sequence: output
                            .transcript
                            .and_then(|t| t.durable.through_sequence),
                        ready: false,
                    },
                );
            }
            PendingObservation::Ack { id, close } => {
                if decode_subscription_close_result(value)?.subscription_id != *id {
                    return Err(invalid("Subscription acknowledgement identity mismatch"));
                }
                if *close {
                    self.0.remove(id);
                }
            }
            PendingObservation::Page { input, session } => {
                let page = decode_session_transcript_page(value)?;
                if page.session_id != *session
                    || page.direction != input.direction
                    || page.through_sequence != input.through_sequence
                    || page.raw_bytes > input.max_bytes
                    || input.cursor.is_some() && page.next_cursor == input.cursor
                {
                    return Err(invalid(
                        "Transcript page correlation changed or cursor did not advance",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn accept(&mut self, frame: &ObservationFrame, epoch: &str) -> Result<()> {
        let envelope = frame.envelope();
        let state = self
            .0
            .get_mut(envelope.subscription_id)
            .ok_or_else(|| invalid("Frame for unknown or closed subscription"))?;
        if envelope.host_epoch != epoch || envelope.session_id.is_some_and(|id| id != state.session)
        {
            return Err(invalid("Subscription frame identity mismatch"));
        }
        if !state.ready {
            return Err(invalid("Subscription frame arrived before ready"));
        }
        if let Some(sequence) = envelope.sequence {
            if sequence != state.next_sequence {
                return Err(invalid("Subscription sequence gap or duplicate"));
            }
            state.next_sequence += 1;
        }
        match frame {
            ObservationFrame::Projection(frame) => {
                let SessionProjectionFrame::SessionProjection { snapshot, .. } = frame.as_ref();
                if snapshot.projection_revision <= state.projection_revision {
                    return Err(invalid("Subscription projection revision did not advance"));
                }
                state.projection_revision = snapshot.projection_revision;
            }
            ObservationFrame::Transcript(TranscriptAdvancedFrame::TranscriptAdvanced {
                through_sequence,
                ..
            }) => {
                if state
                    .through_sequence
                    .is_some_and(|previous| *through_sequence <= previous)
                {
                    return Err(invalid("Subscription transcript watermark did not advance"));
                }
                state.through_sequence = Some(*through_sequence);
            }
            _ => {}
        }
        if frame.is_closed() {
            self.0.remove(envelope.subscription_id);
        }
        Ok(())
    }
}

fn invalid(message: &str) -> ProtocolError {
    ProtocolError::invalid(message)
}
