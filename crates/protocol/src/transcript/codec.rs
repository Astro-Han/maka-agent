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

use super::fragment::decode_fragment;
use super::*;
use crate::{
    ProtocolError, Result,
    codec::{count, exact, record, string},
};
use serde_json::Value;

pub(super) fn ensure(valid: bool, message: &str) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(ProtocolError::invalid(message))
    }
}
fn nullable_count(v: &Value) -> Result<Option<u64>> {
    if v.is_null() {
        Ok(None)
    } else {
        count(v, "transcript count").map(Some)
    }
}
fn cursor(v: &Value) -> Result<Option<String>> {
    if v.is_null() {
        return Ok(None);
    }
    let s = v
        .as_str()
        .ok_or_else(|| ProtocolError::invalid("Invalid cursor"))?;
    ensure(
        !s.is_empty() && s.len() <= SESSION_TRANSCRIPT_CURSOR_MAX_BYTES,
        "Invalid cursor",
    )?;
    Ok(Some(s.to_owned()))
}
pub(super) fn source(v: &Value) -> Result<SessionTranscriptPageSource> {
    serde_json::from_value(v.clone())
        .map_err(|_| ProtocolError::invalid("Invalid transcript source"))
}
fn direction(v: &Value) -> Result<SessionTranscriptPageDirection> {
    serde_json::from_value(v.clone())
        .map_err(|_| ProtocolError::invalid("Invalid transcript direction"))
}

pub fn decode_session_transcript_page_input(v: &Value) -> Result<SessionTranscriptPageInput> {
    exact(
        record(v, "transcript page input")?,
        &[
            "subscriptionId",
            "source",
            "direction",
            "throughSequence",
            "cursor",
            "anchorSequence",
            "maxBytes",
        ],
    )?;
    let cursor = cursor(&v["cursor"])?;
    let anchor_sequence = nullable_count(&v["anchorSequence"])?;
    ensure(
        cursor.is_none() || anchor_sequence.is_none(),
        "Cursor and anchor are mutually exclusive",
    )?;
    let max_bytes = count(&v["maxBytes"], "maxBytes")?;
    ensure(
        (1..=SESSION_TRANSCRIPT_PAGE_MAX_BYTES).contains(&max_bytes),
        "Invalid page byte limit",
    )?;
    Ok(SessionTranscriptPageInput {
        subscription_id: string(&v["subscriptionId"], "subscriptionId", 128)?,
        source: source(&v["source"])?,
        direction: direction(&v["direction"])?,
        through_sequence: nullable_count(&v["throughSequence"])?,
        cursor,
        anchor_sequence,
        max_bytes,
    })
}

pub fn decode_session_transcript_page(v: &Value) -> Result<SessionTranscriptPage> {
    ensure(
        serde_json::to_vec(v)
            .map_err(|e| ProtocolError::invalid(e.to_string()))?
            .len()
            <= SESSION_TRANSCRIPT_PAGE_RESULT_MAX_BYTES,
        "Transcript page exceeds encoded byte limit",
    )?;
    exact(
        record(v, "transcript page")?,
        &[
            "kind",
            "sessionId",
            "source",
            "direction",
            "throughSequence",
            "rawBytes",
            "fragments",
            "rangeBoundarySequence",
            "protectedTurnSequence",
            "nextCursor",
        ],
    )?;
    ensure(v["kind"] == "page", "Invalid transcript page kind")?;
    let session_id = string(&v["sessionId"], "sessionId", 128)?;
    ensure(
        session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "Invalid sessionId",
    )?;
    let source = source(&v["source"])?;
    let direction = direction(&v["direction"])?;
    let through_sequence = nullable_count(&v["throughSequence"])?;
    let entries = v["fragments"]
        .as_array()
        .filter(|a| a.len() <= SESSION_TRANSCRIPT_PAGE_MAX_MESSAGES)
        .ok_or_else(|| ProtocolError::invalid("Invalid transcript fragments"))?;
    let mut fragments = Vec::with_capacity(entries.len());
    let mut bytes = 0;
    for entry in entries {
        let (fragment, size) = decode_fragment(entry, source, through_sequence)?;
        if let Some(previous) = fragments.last().map(SessionTranscriptFragment::identity) {
            ensure(
                match direction {
                    SessionTranscriptPageDirection::Older => fragment.identity() < previous,
                    SessionTranscriptPageDirection::Newer => fragment.identity() > previous,
                },
                "Invalid transcript fragment order",
            )?;
        }
        bytes += size;
        fragments.push(fragment);
    }
    let raw_bytes = count(&v["rawBytes"], "rawBytes")?;
    ensure(
        raw_bytes <= SESSION_TRANSCRIPT_PAGE_MAX_BYTES && bytes == raw_bytes,
        "Invalid transcript byte count",
    )?;
    let next_cursor = cursor(&v["nextCursor"])?;
    ensure(
        !fragments.is_empty() || next_cursor.is_none(),
        "Empty page cannot have a cursor",
    )?;
    let range_boundary_sequence = nullable_count(&v["rangeBoundarySequence"])?;
    let protected_turn_sequence = nullable_count(&v["protectedTurnSequence"])?;
    for boundary in [range_boundary_sequence, protected_turn_sequence]
        .into_iter()
        .flatten()
    {
        ensure(
            source == SessionTranscriptPageSource::Durable
                && through_sequence.is_some_and(|w| boundary <= w),
            "Invalid transcript range boundary",
        )?;
    }
    Ok(SessionTranscriptPage {
        session_id,
        source,
        direction,
        through_sequence,
        raw_bytes,
        fragments,
        range_boundary_sequence,
        protected_turn_sequence,
        next_cursor,
    })
}

pub fn assert_page_for_input(
    input: &SessionTranscriptPageInput,
    page: &SessionTranscriptPage,
) -> Result<()> {
    ensure(
        page.source == input.source
            && page.direction == input.direction
            && page.through_sequence == input.through_sequence
            && page.raw_bytes <= input.max_bytes,
        "Transcript page does not match request",
    )
}

pub fn decode_session_transcript_bootstrap(v: &Value) -> Result<SessionTranscriptBootstrap> {
    exact(
        record(v, "transcript bootstrap")?,
        &[
            "throughSequence",
            "overlayMessageCount",
            "durable",
            "overlay",
        ],
    )?;
    let result = SessionTranscriptBootstrap {
        through_sequence: nullable_count(&v["throughSequence"])?,
        overlay_message_count: count(&v["overlayMessageCount"], "overlayMessageCount")?,
        durable: decode_session_transcript_page(&v["durable"])?,
        overlay: decode_session_transcript_page(&v["overlay"])?,
    };
    ensure(
        result.overlay_message_count <= SESSION_TRANSCRIPT_OVERLAY_MAX_MESSAGES,
        "Overlay exceeds message limit",
    )?;
    ensure(
        result.durable.source == SessionTranscriptPageSource::Durable
            && result.overlay.source == SessionTranscriptPageSource::Overlay
            && result.durable.direction == SessionTranscriptPageDirection::Older
            && result.overlay.direction == SessionTranscriptPageDirection::Older
            && result.durable.through_sequence == result.through_sequence
            && result.overlay.through_sequence == result.through_sequence,
        "Invalid transcript bootstrap correlation",
    )?;
    ensure(
        result.durable.raw_bytes + result.overlay.raw_bytes
            <= SESSION_TRANSCRIPT_BOOTSTRAP_MAX_BYTES,
        "Bootstrap exceeds byte limit",
    )?;
    Ok(result)
}

/// Checks the enclosing subscription identity and requested combined tail budget.
/// Individual pages must already have passed their wire decoder.
pub fn validate_bootstrap_for_input(
    bootstrap: &SessionTranscriptBootstrap,
    session_id: &str,
    max_bytes: u64,
) -> Result<()> {
    ensure(
        (2..=SESSION_TRANSCRIPT_BOOTSTRAP_MAX_BYTES).contains(&max_bytes)
            && bootstrap.durable.session_id == session_id
            && bootstrap.overlay.session_id == session_id
            && bootstrap
                .durable
                .raw_bytes
                .checked_add(bootstrap.overlay.raw_bytes)
                .is_some_and(|bytes| bytes <= max_bytes),
        "Bootstrap does not match subscription request",
    )
}

pub fn decode_session_transcript_overlay_release_input(
    v: &Value,
) -> Result<SessionTranscriptOverlayReleaseInput> {
    exact(record(v, "overlay release")?, &["subscriptionId"])?;
    Ok(SessionTranscriptOverlayReleaseInput {
        subscription_id: string(&v["subscriptionId"], "subscriptionId", 128)?,
    })
}
pub fn decode_session_transcript_overlay_release_result(
    v: &Value,
) -> Result<SessionTranscriptOverlayReleaseResult> {
    decode_session_transcript_overlay_release_input(v)
}

pub fn assert_overlay_release_for_input(
    input: &SessionTranscriptOverlayReleaseInput,
    output: &SessionTranscriptOverlayReleaseResult,
) -> Result<()> {
    ensure(
        input.subscription_id == output.subscription_id,
        "Transcript overlay release identity changed",
    )
}
