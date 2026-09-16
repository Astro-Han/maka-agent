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

use super::{cursor::Position, *};
use SessionTranscriptPageDirection::{Newer, Older};
use base64::{Engine, engine::general_purpose::STANDARD};
use maka_event_log::transcript::{TranscriptDirection, TranscriptRead, TranscriptRowHeader};

pub(super) async fn read(
    state: &Transcript,
    log: &EventLog,
    input: &SessionTranscriptPageInput,
) -> Result<SessionTranscriptPage> {
    let mut result = SessionTranscriptPage {
        session_id: state.session_id.clone(),
        direction: input.direction,
        through_sequence: input.through_sequence,
        raw_bytes: 0,
        fragments: vec![],
        range_boundary_sequence: None,
        protected_turn_sequence: None,
        next_cursor: None,
    };
    let Some(mut position) = start(state, input)? else {
        return Ok(result);
    };
    let Some(mut row) = header(state, log, input, position.position).await? else {
        if input.cursor.is_some() {
            return Err(TranscriptError::InvalidRequest("cursor row is unavailable"));
        }
        return Ok(result);
    };
    if position.boundary.is_none() {
        let bounds = log
            .transcript_turn_bounds(
                &state.session_id,
                &row.turn_id,
                input.through_sequence.unwrap(),
            )
            .await?
            .ok_or(TranscriptError::InvalidRequest("turn range unavailable"))?;
        // One Turn per range is sufficient for bounded whole-Turn navigation.
        // Oversized Turns deliberately use the TS null-boundary fallback: clients
        // can assemble individual rows without claiming a bounded complete Turn.
        if bounds.rows <= SESSION_TRANSCRIPT_RANGE_MAX_MESSAGES as u64
            && bounds.bytes <= SESSION_TRANSCRIPT_RANGE_MAX_BYTES
        {
            position.boundary = Some(if input.direction == Older {
                bounds.first
            } else {
                bounds.last
            });
            position.protected = Some(if input.direction == Older {
                row.sequence
            } else {
                bounds.last
            });
        }
    }
    result.range_boundary_sequence = position.boundary;
    result.protected_turn_sequence = position.protected;
    loop {
        let (offset, length, continuation) = slice(
            row.total_bytes,
            input.direction,
            position.offset,
            input.max_bytes - result.raw_bytes,
        )?;
        let bytes = log
            .transcript_fragment(&state.session_id, row.sequence, offset, length)
            .await?;
        result.fragments.push(SessionTranscriptFragment {
            sequence: row.sequence,
            byte_offset: offset,
            total_bytes: row.total_bytes,
            payload_digest: Some(row.digest),
            data: STANDARD.encode(bytes),
        });
        result.raw_bytes += length;
        if let Some(offset) = continuation {
            position.position = row.sequence;
            position.offset = Some(offset);
            result.next_cursor = Some(state.cursor.encode(input, position)?);
            break;
        }
        let boundary_reached = position.boundary == Some(row.sequence);
        let next = step(row.sequence, input.direction);
        let Some(next) = next else {
            break;
        };
        let Some(next_row) = header(state, log, input, next).await? else {
            break;
        };
        position.position = next_row.sequence;
        position.offset = None;
        if boundary_reached {
            position.boundary = None;
            position.protected = None;
        }
        if boundary_reached
            || result.raw_bytes == input.max_bytes
            || result.fragments.len() == SESSION_TRANSCRIPT_PAGE_MAX_MESSAGES
        {
            result.next_cursor = Some(state.cursor.encode(input, position)?);
            break;
        }
        row = next_row;
    }
    Ok(result)
}

async fn header(
    state: &Transcript,
    log: &EventLog,
    input: &SessionTranscriptPageInput,
    position: u64,
) -> Result<Option<TranscriptRowHeader>> {
    let Some(through) = input.through_sequence else {
        return Ok(None);
    };
    let rows = log
        .transcript_headers(
            &state.session_id,
            &TranscriptRead {
                through,
                position,
                direction: if input.direction == Older {
                    TranscriptDirection::Older
                } else {
                    TranscriptDirection::Newer
                },
                limit: 1,
            },
        )
        .await?;
    Ok(rows.into_iter().next())
}

fn start(state: &Transcript, input: &SessionTranscriptPageInput) -> Result<Option<Position>> {
    if let Some(token) = &input.cursor {
        return state.cursor.decode(input, token).map(Some);
    }
    let last = input.through_sequence;
    let Some(last) = last else {
        return Ok(None);
    };
    let position = match (input.direction, input.anchor_sequence) {
        (Older, None) => Some(last),
        (Newer, None) => Some(0),
        (direction, Some(anchor)) => step(anchor, direction),
    };
    Ok(position.filter(|p| *p <= last).map(|position| Position {
        position,
        offset: None,
        boundary: None,
        protected: None,
    }))
}
fn step(position: u64, direction: SessionTranscriptPageDirection) -> Option<u64> {
    match direction {
        Older => position.checked_sub(1),
        Newer => position.checked_add(1),
    }
}
fn slice(
    total: u64,
    direction: SessionTranscriptPageDirection,
    offset: Option<u64>,
    budget: u64,
) -> Result<(u64, u64, Option<u64>)> {
    let invalid = || TranscriptError::InvalidRequest("invalid fragment offset");
    if total == 0 || budget == 0 {
        return Err(invalid());
    }
    match direction {
        Older => {
            let end = offset.unwrap_or(total);
            if end == 0 || end > total {
                return Err(invalid());
            }
            let length = end.min(budget);
            let start = end - length;
            Ok((start, length, (start > 0).then_some(start)))
        }
        Newer => {
            let start = offset.unwrap_or(0);
            if start >= total {
                return Err(invalid());
            }
            let length = (total - start).min(budget);
            let end = start + length;
            Ok((start, length, (end < total).then_some(end)))
        }
    }
}
