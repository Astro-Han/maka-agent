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

use super::codec::{cursor, ensure, nullable_count};
use crate::{
    Result,
    codec::{count, exact, record, string},
};
use serde::Serialize;
use serde_json::Value;

pub const SEARCH_QUERY_MAX_BYTES: usize = 512;
pub const SEARCH_PREVIEW_MAX_BYTES: usize = 384;
pub const SEARCH_MAX_MATCHES: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSearchInput {
    pub subscription_id: String,
    pub through_sequence: Option<u64>,
    pub query: String,
    pub include_internal: bool,
    pub cursor: Option<String>,
    pub max_matches: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSearchMatch {
    pub sequence: u64,
    pub preview: String,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSearchResult {
    pub session_id: String,
    pub through_sequence: Option<u64>,
    pub matches: Vec<TranscriptSearchMatch>,
    /// More scanning may be needed even when this batch has no matches.
    pub next_cursor: Option<String>,
}

pub fn decode_transcript_search_input(v: &Value) -> Result<TranscriptSearchInput> {
    exact(
        record(v, "transcript search input")?,
        &[
            "subscriptionId",
            "throughSequence",
            "query",
            "includeInternal",
            "cursor",
            "maxMatches",
        ],
    )?;
    let max_matches = count(&v["maxMatches"], "maxMatches")?;
    ensure(
        (1..=SEARCH_MAX_MATCHES as u64).contains(&max_matches),
        "Invalid search match limit",
    )?;
    ensure(
        v["includeInternal"].is_boolean(),
        "Invalid search visibility",
    )?;
    let query = string(&v["query"], "query", SEARCH_QUERY_MAX_BYTES)?;
    ensure(
        query.len() <= SEARCH_QUERY_MAX_BYTES,
        "Search query exceeds byte limit",
    )?;
    Ok(TranscriptSearchInput {
        subscription_id: string(&v["subscriptionId"], "subscriptionId", 128)?,
        through_sequence: nullable_count(&v["throughSequence"])?,
        query,
        include_internal: v["includeInternal"].as_bool().unwrap(),
        cursor: cursor(&v["cursor"])?,
        max_matches: max_matches as usize,
    })
}
pub fn decode_transcript_search_result(v: &Value) -> Result<TranscriptSearchResult> {
    exact(
        record(v, "transcript search result")?,
        &["sessionId", "throughSequence", "matches", "nextCursor"],
    )?;
    let session_id = string(&v["sessionId"], "sessionId", 128)?;
    ensure(
        session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "Invalid sessionId",
    )?;
    let through_sequence = nullable_count(&v["throughSequence"])?;
    let rows = v["matches"]
        .as_array()
        .ok_or_else(|| crate::ProtocolError::invalid("Invalid search matches"))?;
    ensure(rows.len() <= SEARCH_MAX_MATCHES, "Too many search matches")?;
    let mut matches = Vec::with_capacity(rows.len());
    let mut previous = None;
    for row in rows {
        exact(
            record(row, "transcript search match")?,
            &["sequence", "preview"],
        )?;
        let sequence = count(&row["sequence"], "sequence")?;
        ensure(
            Some(sequence) <= through_sequence
                && previous.is_none_or(|previous| sequence > previous),
            "Invalid search match order",
        )?;
        let preview = string(&row["preview"], "preview", SEARCH_PREVIEW_MAX_BYTES)?;
        ensure(
            preview.len() <= SEARCH_PREVIEW_MAX_BYTES,
            "Search preview exceeds byte limit",
        )?;
        matches.push(TranscriptSearchMatch { sequence, preview });
        previous = Some(sequence);
    }
    let next_cursor = cursor(&v["nextCursor"])?;
    ensure(
        through_sequence.is_some() || next_cursor.is_none(),
        "Empty history cannot continue",
    )?;
    Ok(TranscriptSearchResult {
        session_id,
        through_sequence,
        matches,
        next_cursor,
    })
}
pub fn validate_search_result(
    input: &TranscriptSearchInput,
    result: &TranscriptSearchResult,
    session: &str,
) -> Result<()> {
    ensure(
        result.session_id == session
            && result.through_sequence == input.through_sequence
            && result.matches.len() <= input.max_matches
            && result
                .next_cursor
                .as_ref()
                .is_none_or(|cursor| input.cursor.as_ref() != Some(cursor)),
        "Transcript search response does not match request",
    )
}
