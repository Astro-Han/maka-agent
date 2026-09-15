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

use super::super::{Host, HostError, sessions};
use super::streams::Streams;
use super::transcript::{PreparedTranscript, TranscriptAccess};
use crate::{execution::snapshot, session::SessionConfiguration};
use maka_event_log::observation::{SessionObservation, SessionProjection};
use maka_protocol::subscription::*;
use serde_json::Value;

mod events;

struct Pending {
    through: u64,
    snapshot: SessionObservationSnapshot,
}

pub(super) struct Delivery {
    pub(super) version: Option<maka_event_log::observation::ObservationVersion>,
    epoch: String,
    id: String,
    session_id: String,
    sequence: u64,
    cursor: u64,
    snapshot: SessionObservationSnapshot,
    streams: Streams,
    pending: Option<Pending>,
    pub(super) transcript: Option<TranscriptAccess>,
}

impl Delivery {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn resource_changed(
        &mut self,
        change: &maka_event_log::shell_runs::ShellChange,
    ) -> Result<Option<Value>, HostError> {
        if change.session_id != self.session_id {
            return Ok(None);
        }
        let frame = ResourceObservationFrame::DomainChanged {
            host_epoch: self.epoch.clone(),
            subscription_id: self.id.clone(),
            sequence: self.sequence,
            session_id: self.session_id.clone(),
            domain: ResourceDomain::RuntimeResource,
            resources: vec![ResourceChange {
                source_session_id: change.session_id.clone(),
                resource_ref: format!(
                    "{}{}",
                    maka_presentation::shell::RESOURCE_REF_PREFIX,
                    change.id
                ),
            }],
        };
        let value = serde_json::to_value(frame)?;
        decode_resource_observation_frame(&value)?;
        self.sequence += 1;
        Ok(Some(value))
    }

    pub fn open(
        epoch: &str,
        id: &str,
        observation: SessionObservation<SessionConfiguration>,
        prepared: Option<PreparedTranscript>,
    ) -> Result<(Self, SubscriptionOpenResult), HostError> {
        let streams = Streams::bootstrap(
            observation
                .root_turn
                .as_ref()
                .map(|turn| turn.root_invocation()),
            observation.active_streams,
        );
        let cursor = observation.through_sequence;
        let snapshot = project(
            epoch,
            1,
            SessionProjection {
                session: observation.session,
                root_turn: observation.root_turn,
                through_sequence: cursor,
                pending_interactions: observation.pending_interactions,
                message_queue: observation.message_queue,
            },
        )?;
        snapshot.validate()?;
        let (transcript, bootstrap) = prepared.map_or((None, None), |prepared| {
            (Some(prepared.access), Some(prepared.bootstrap))
        });
        let output = SubscriptionOpenResult::new(
            epoch.into(),
            id.into(),
            1,
            snapshot.clone(),
            streams.identities(),
            bootstrap,
        );
        Ok((
            Self {
                epoch: epoch.into(),
                id: id.into(),
                session_id: snapshot.session.session_id.clone(),
                sequence: 1,
                cursor,
                snapshot,
                version: None,
                streams,
                pending: None,
                transcript,
            },
            output,
        ))
    }

    pub async fn poll(&mut self, host: &Host) -> Result<(Vec<Value>, bool), HostError> {
        let continuing = self.pending.is_some();
        if self.pending.is_none() {
            let observation = host
                .log
                .session_projection::<SessionConfiguration>(&self.session_id)
                .await?
                .ok_or("observed Session disappeared")?;
            self.pending = Some(Pending {
                through: observation.through_sequence,
                snapshot: project(&self.epoch, self.snapshot.projection_revision, observation)?,
            });
        }
        let through = self.pending.as_ref().expect("pending fence").through;
        let cursor = self.cursor;
        let page = host
            .log
            .session_stream_events(&self.session_id, cursor, through, 16, 128 * 1024)
            .await?;
        let (mut frames, more) = self.deliver_page(page)?;
        if more {
            return Ok((frames, true));
        }
        // A watch wakeup may have been consumed while finishing an older paged
        // fence. Take a fresh snapshot once before declaring catch-up idle.
        let mut pending = false;
        if let Some(transcript) = &mut self.transcript {
            if let Some(through_sequence) = transcript
                .poll(host.log.clone(), self.session_id.clone())
                .await?
            {
                let frame = TranscriptAdvancedFrame::TranscriptAdvanced {
                    host_epoch: self.epoch.clone(),
                    subscription_id: self.id.clone(),
                    sequence: self.sequence,
                    session_id: self.session_id.clone(),
                    through_sequence,
                };
                frame.validate()?;
                frames.push(serde_json::to_value(frame)?);
                self.sequence += 1;
            }
            pending = transcript.is_pending();
        }
        Ok((frames, continuing || pending))
    }
}

pub(crate) fn project(
    epoch: &str,
    revision: u64,
    observation: SessionProjection<SessionConfiguration>,
) -> Result<SessionObservationSnapshot, HostError> {
    let interactions = SessionInteractionProjection::from_records(
        &observation.pending_interactions,
        &observation.session.id,
    )?;
    let session = sessions::projection::project(observation.session);
    Ok(SessionObservationSnapshot::new(
        SessionObservationIdentity {
            session_id: session.id,
            metadata_revision: session.revision,
            status: session.status,
            created_at: session.created_at,
            is_archived: session.is_archived,
        },
        revision,
        observation
            .root_turn
            .map(|turn| snapshot::project(turn).snapshot),
        None,
        super::super::messages::projection::subscription(epoch, &observation.message_queue),
        interactions,
    ))
}
