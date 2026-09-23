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

//! Bounded presentation reads over an already prepared canonical log fence.
mod cursor;
mod page;

use maka_event_log::{EventLog, StoreError};
use maka_protocol::transcript::*;

#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    #[error("invalid transcript request: {0}")]
    InvalidRequest(&'static str),
    #[error("transcript exceeds available capacity")]
    Capacity,
    #[error(transparent)]
    Persistence(#[from] StoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
type Result<T> = std::result::Result<T, TranscriptError>;
pub struct Transcript {
    subscription_id: String,
    session_id: String,
    opened_watermark: Option<u64>,
    announced_watermark: Option<u64>,
    cursor: cursor::Signer,
}

impl Transcript {
    pub fn new(
        subscription_id: String,
        session_id: String,
        opened_watermark: Option<u64>,
    ) -> Result<Self> {
        if opened_watermark.is_some_and(|w| w > 9_007_199_254_740_991) {
            return Err(TranscriptError::InvalidRequest("unsafe watermark"));
        }
        let cursor = cursor::Signer::new(&subscription_id, &session_id);
        Ok(Self {
            subscription_id,
            session_id,
            opened_watermark,
            announced_watermark: opened_watermark,
            cursor,
        })
    }

    pub fn watermark(&self) -> Option<u64> {
        self.announced_watermark
    }

    /// Caller sends advancement on the ordered subscription wire before exposing it.
    pub fn advance(&mut self, watermark: Option<u64>) -> Result<bool> {
        if watermark.is_some_and(|w| w > 9_007_199_254_740_991)
            || watermark < self.announced_watermark
        {
            return Err(TranscriptError::InvalidRequest(
                "watermark moved backwards or is unsafe",
            ));
        }
        let changed = watermark != self.announced_watermark;
        self.announced_watermark = watermark;
        Ok(changed)
    }

    pub async fn bootstrap(
        &self,
        log: &EventLog,
        max_bytes: u64,
    ) -> Result<SessionTranscriptBootstrap> {
        if !(2..=SESSION_TRANSCRIPT_BOOTSTRAP_MAX_BYTES).contains(&max_bytes) {
            return Err(TranscriptError::InvalidRequest("invalid bootstrap budget"));
        }
        let durable = self
            .page(
                log,
                &SessionTranscriptPageInput {
                    subscription_id: self.subscription_id.clone(),
                    direction: SessionTranscriptPageDirection::Older,
                    through_sequence: self.opened_watermark,
                    cursor: None,
                    anchor_sequence: None,
                    max_bytes,
                },
            )
            .await?;
        Ok(SessionTranscriptBootstrap { durable })
    }

    pub async fn page(
        &self,
        log: &EventLog,
        input: &SessionTranscriptPageInput,
    ) -> Result<SessionTranscriptPage> {
        if input.subscription_id != self.subscription_id
            || !(1..=SESSION_TRANSCRIPT_PAGE_MAX_BYTES).contains(&input.max_bytes)
            || (input.cursor.is_some() && input.anchor_sequence.is_some())
            || (input.anchor_sequence.is_some()
                && input.direction == SessionTranscriptPageDirection::Older)
            || input
                .anchor_sequence
                .is_some_and(|s| s > 9_007_199_254_740_991)
        {
            return Err(TranscriptError::InvalidRequest(
                "invalid subscription, anchor or budget",
            ));
        }
        if input.through_sequence > self.announced_watermark {
            return Err(TranscriptError::InvalidRequest("watermark not announced"));
        }
        page::read(self, log, input).await
    }

    pub async fn search(
        &self,
        log: &EventLog,
        input: &TranscriptSearchInput,
    ) -> Result<TranscriptSearchResult> {
        decode_transcript_search_input(&serde_json::to_value(input)?)
            .map_err(|_| TranscriptError::InvalidRequest("invalid search input"))?;
        if input.subscription_id != self.subscription_id
            || input.through_sequence > self.announced_watermark
        {
            return Err(TranscriptError::InvalidRequest(
                "invalid subscription or unannounced watermark",
            ));
        }
        let after = input
            .cursor
            .as_ref()
            .map(|cursor| self.cursor.decode_search(input, cursor))
            .transpose()?
            .unwrap_or(0);
        let Some(through) = input.through_sequence else {
            if input.cursor.is_some() {
                return Err(TranscriptError::InvalidRequest("empty history cursor"));
            }
            return Ok(TranscriptSearchResult {
                session_id: self.session_id.clone(),
                through_sequence: None,
                matches: vec![],
                next_cursor: None,
            });
        };
        let batch = log
            .search_transcript(
                &self.session_id,
                maka_event_log::transcript::search::TranscriptSearch {
                    through,
                    after,
                    query: input.query.clone(),
                    include_internal: input.include_internal,
                    max_matches: input.max_matches,
                },
            )
            .await?;
        Ok(TranscriptSearchResult {
            session_id: self.session_id.clone(),
            through_sequence: Some(through),
            matches: batch
                .matches
                .into_iter()
                .map(|(sequence, preview)| TranscriptSearchMatch { sequence, preview })
                .collect(),
            next_cursor: batch
                .next_after
                .map(|after| self.cursor.encode_search(input, after))
                .transpose()?,
        })
    }
}
