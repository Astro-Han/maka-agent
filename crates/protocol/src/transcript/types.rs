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

use serde::{Deserialize, Serialize, ser::SerializeStruct};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionTranscriptPageSource {
    Durable,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionTranscriptPageDirection {
    Older,
    Newer,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum SessionTranscriptFragment {
    Durable {
        sequence: u64,
        byte_offset: u64,
        total_bytes: u64,
        payload_digest: Option<String>,
        data: String,
    },
    Overlay {
        message_index: u64,
        byte_offset: u64,
        total_bytes: u64,
        data: String,
    },
}

impl SessionTranscriptFragment {
    pub fn identity(&self) -> u64 {
        match self {
            Self::Durable { sequence, .. } => *sequence,
            Self::Overlay { message_index, .. } => *message_index,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionTranscriptPage {
    pub session_id: String,
    pub source: SessionTranscriptPageSource,
    pub direction: SessionTranscriptPageDirection,
    pub through_sequence: Option<u64>,
    pub raw_bytes: u64,
    pub fragments: Vec<SessionTranscriptFragment>,
    pub range_boundary_sequence: Option<u64>,
    pub protected_turn_sequence: Option<u64>,
    pub next_cursor: Option<String>,
}

impl Serialize for SessionTranscriptPage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("SessionTranscriptPage", 10)?;
        s.serialize_field("kind", "page")?;
        s.serialize_field("sessionId", &self.session_id)?;
        s.serialize_field("source", &self.source)?;
        s.serialize_field("direction", &self.direction)?;
        s.serialize_field("throughSequence", &self.through_sequence)?;
        s.serialize_field("rawBytes", &self.raw_bytes)?;
        s.serialize_field("fragments", &self.fragments)?;
        s.serialize_field("rangeBoundarySequence", &self.range_boundary_sequence)?;
        s.serialize_field("protectedTurnSequence", &self.protected_turn_sequence)?;
        s.serialize_field("nextCursor", &self.next_cursor)?;
        s.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTranscriptPageInput {
    pub subscription_id: String,
    pub source: SessionTranscriptPageSource,
    pub direction: SessionTranscriptPageDirection,
    pub through_sequence: Option<u64>,
    pub cursor: Option<String>,
    pub anchor_sequence: Option<u64>,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTranscriptBootstrap {
    pub through_sequence: Option<u64>,
    pub overlay_message_count: u64,
    pub durable: SessionTranscriptPage,
    pub overlay: SessionTranscriptPage,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTranscriptOverlayReleaseInput {
    pub subscription_id: String,
}
pub type SessionTranscriptOverlayReleaseResult = SessionTranscriptOverlayReleaseInput;
