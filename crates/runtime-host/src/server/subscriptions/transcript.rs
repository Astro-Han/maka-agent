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

use crate::{
    session::SessionConfiguration,
    transcript::{Transcript, TranscriptError},
};
use maka_event_log::{EventLog, StoreError, observation::SessionObservation};
use maka_protocol::subscription::TranscriptPolicy;
use maka_protocol::transcript::SessionTranscriptBootstrap;
use maka_protocol::{OperationError, OperationErrorCode as Code};
use std::sync::Arc;

pub(super) struct PreparedTranscript {
    pub access: TranscriptAccess,
    pub bootstrap: SessionTranscriptBootstrap,
}

/// Pending index work exists only while the subscription has transcript access.
pub(super) struct TranscriptAccess {
    pub state: Transcript,
    preparation: Preparation,
}

enum Preparation {
    Idle,
    Pending(u64),
    Unavailable,
}

impl TranscriptAccess {
    pub fn catch_up_to(&mut self, through: u64) {
        if maka_presentation::watermark(through)
            .is_ok_and(|watermark| Some(watermark) <= self.state.watermark())
        {
            return;
        }
        self.preparation = match self.preparation {
            Preparation::Idle => Preparation::Pending(through),
            Preparation::Pending(fence) => Preparation::Pending(fence.max(through)),
            // The oversized boundary is immutable: later fences cannot repair it.
            Preparation::Unavailable => Preparation::Unavailable,
        };
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.preparation, Preparation::Pending(_))
    }

    pub fn is_unavailable(&self) -> bool {
        matches!(self.preparation, Preparation::Unavailable)
    }

    pub async fn poll(
        &mut self,
        log: Arc<EventLog>,
        session: String,
    ) -> Result<Option<u64>, crate::server::HostError> {
        let Preparation::Pending(fence) = self.preparation else {
            return Ok(None);
        };
        match log.prepare_transcript(&session, fence, 32).await {
            Ok(true) => (),
            Ok(false) => return Ok(None),
            Err(StoreError::PrefixTooLarge)
            | Err(StoreError::Projection(maka_presentation::ProjectionError::TooLarge)) => {
                // Preserve the last announced fence and keep other observations alive.
                self.preparation = Preparation::Unavailable;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        }
        let through = maka_presentation::watermark(fence)?;
        let advanced = self.state.advance(Some(through))?;
        self.preparation = Preparation::Idle;
        Ok(advanced.then_some(through))
    }
}

pub(super) async fn prepare(
    log: &EventLog,
    observation: &SessionObservation<SessionConfiguration>,
    id: String,
    policy: &TranscriptPolicy,
) -> Result<Option<PreparedTranscript>, OperationError> {
    let TranscriptPolicy::Tail { max_bytes } = policy else {
        return Ok(None);
    };
    let session = &observation.session.id;
    let source_fence = observation.through_sequence;
    if !log
        .prepare_transcript(session, source_fence, 32)
        .await
        .map_err(store_error)?
    {
        return Err(OperationError {
            code: Code::TranscriptPreparing,
            message: "Transcript preparation is progressing".into(),
        });
    }
    let through =
        maka_presentation::watermark(source_fence).map_err(|error| store_error(error.into()))?;
    let state = Transcript::new(id, session.clone(), Some(through)).map_err(operation_error)?;
    let bootstrap = state
        .bootstrap(log, *max_bytes)
        .await
        .map_err(operation_error)?;
    Ok(Some(PreparedTranscript {
        access: TranscriptAccess {
            state,
            preparation: Preparation::Idle,
        },
        bootstrap,
    }))
}

pub(super) fn store_error(error: StoreError) -> OperationError {
    operation_error(TranscriptError::Persistence(error))
}
pub(super) fn operation_error(error: TranscriptError) -> OperationError {
    let code = match &error {
        TranscriptError::InvalidRequest(_) => Code::InvalidRequest,
        TranscriptError::Capacity => Code::OperationConflict,
        TranscriptError::Persistence(StoreError::Projection(_))
        | TranscriptError::Persistence(StoreError::PrefixTooLarge) => Code::OperationUnavailable,
        TranscriptError::Persistence(_) => Code::PersistenceFailed,
        TranscriptError::Json(_) => Code::InternalFailure,
    };
    OperationError {
        code,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_runtime::event::{EventWrite, Fact, Invocation, InvocationInput, RuntimeEvent};

    fn access() -> TranscriptAccess {
        TranscriptAccess {
            state: Transcript::new("subscription".into(), "session".into(), Some(0)).unwrap(),
            preparation: Preparation::Idle,
        }
    }

    #[tokio::test]
    async fn capacity_failure_preserves_fence_and_stops_retrying_immutable_boundary() {
        // A large projected message and oversized canonical evidence exercise
        // the two capacity errors that must not terminate the connection.
        for bytes in [9 * 1024 * 1024, 17 * 1024 * 1024] {
            let temp = tempfile::tempdir().unwrap();
            let log = Arc::new(
                EventLog::open(&temp.path().join("log.sqlite"))
                    .await
                    .unwrap(),
            );
            let event = EventWrite::plain(RuntimeEvent::new(
                Invocation {
                    session_id: "session".into(),
                    turn_id: "turn".into(),
                    run_id: "run".into(),
                    invocation_id: "invocation".into(),
                },
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        source_messages: Vec::new(),
                        content: "x".repeat(bytes).into(),
                        request_fingerprint: None,
                    },
                },
            ))
            .unwrap();
            let through = log.append(&event).await.unwrap();
            let mut access = access();
            access.catch_up_to(through);
            assert_eq!(
                access.poll(log.clone(), "session".into()).await.unwrap(),
                None
            );
            assert!(access.is_unavailable());
            assert!(!access.is_pending());
            assert!(!access.state.advance(Some(0)).unwrap());
            // An invalid new fence/session would fail if blocked work retried.
            access.catch_up_to(u64::MAX);
            assert!(!access.is_pending());
            assert_eq!(access.poll(log, "".into()).await.unwrap(), None);
        }
    }

    #[tokio::test]
    async fn invalid_projection_is_not_treated_as_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let log = Arc::new(
            EventLog::open(&temp.path().join("log.sqlite"))
                .await
                .unwrap(),
        );
        let mut access = access();
        access.catch_up_to(u64::MAX);
        assert!(access.poll(log, "session".into()).await.is_err());
        assert!(!access.is_unavailable());
    }
}
