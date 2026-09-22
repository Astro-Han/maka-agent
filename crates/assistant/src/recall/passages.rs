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

use super::{
    rank::{Hit, Source, excluded},
    reader::{Reader, failed},
    types::{Passage, PassageMessage},
};
use maka_plugins::{
    call::Scope,
    session::history::{Chunk, History},
};
use maka_runtime::tools::ToolError;
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;
use unicode_normalization::UnicodeNormalization;

pub(super) async fn build(
    history: Arc<dyn History>,
    call: &Scope,
    sources: &[Source],
    hits: &[Hit],
    terms: &[String],
    span: (usize, usize),
    permit: Arc<OwnedSemaphorePermit>,
) -> Result<Vec<Passage>, ToolError> {
    let mut passages: Vec<_> = hits
        .iter()
        .map(|hit| Passage {
            session_id: sources[hit.session].summary.session.session_id.clone(),
            session_title: sources[hit.session].summary.session.name.clone(),
            anchor_message_id: hit.message_id.clone(),
            anchor_sequence: hit.sequence,
            turn_id: hit.turn_id.clone(),
            messages: Vec::new(),
            matched_terms: terms
                .iter()
                .zip(&hit.frequencies)
                .filter_map(|(term, count)| (*count > 0).then_some(term.clone()))
                .collect(),
            score: hit.score,
            has_more_before: false,
            has_more_after: false,
            truncated: false,
        })
        .collect();
    for (session, source) in sources.iter().enumerate() {
        if !hits.iter().any(|hit| hit.session == session) {
            continue;
        }
        let mut reader = Reader::new(
            history.clone(),
            call.clone(),
            source.summary.session.session_id.clone(),
            source.through,
        );
        let mut position = 0;
        while let Some(message) = reader.next().await? {
            let ordinal = position;
            position += 1;
            if excluded(call, &source.summary.session.session_id, &message) {
                continue;
            }
            let mut targets = Vec::new();
            for (index, hit) in hits
                .iter()
                .enumerate()
                .filter(|(_, hit)| hit.session == session)
            {
                if ordinal < hit.position.saturating_sub(span.0) {
                    passages[index].has_more_before = true;
                } else if ordinal > hit.position + span.1 {
                    passages[index].has_more_after = true;
                } else {
                    let anchor = ordinal == hit.position;
                    targets.push((index, anchor, if anchor { hit.offset } else { 0 }));
                }
            }
            if targets.is_empty() {
                continue;
            }
            let worker = permit.clone();
            let excerpts = tokio::task::spawn_blocking(move || {
                let _worker = worker;
                let text: String = message.text.nfc().collect();
                targets
                    .into_iter()
                    .map(|(index, anchor, offset)| {
                        (index, excerpt(&message, &text, anchor, offset))
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .map_err(failed)?;
            for (index, excerpt) in excerpts {
                passages[index].messages.push(excerpt);
            }
        }
    }
    for passage in &mut passages {
        if !passage.messages.iter().any(|message| message.is_anchor) {
            return Err(failed("Recall anchor is no longer readable"));
        }
        // Keep the anchor first in the byte allocation, then nearest neighbors.
        let anchor = passage
            .messages
            .iter()
            .position(|message| message.is_anchor)
            .unwrap();
        let mut order: Vec<_> = (0..passage.messages.len()).collect();
        order.sort_by_key(|index| index.abs_diff(anchor));
        let mut budget = 12 * 1024;
        let mut keep = std::collections::BTreeSet::new();
        for index in order {
            let size = passage.messages[index].text.len()
                + 256
                + passage.messages[index]
                    .attachments
                    .iter()
                    .map(|a| a.text_bytes() + 128)
                    .sum::<usize>();
            if size <= budget {
                keep.insert(index);
                budget -= size;
            } else {
                passage.truncated = true;
                if index < anchor {
                    passage.has_more_before = true;
                } else {
                    passage.has_more_after = true;
                }
            }
        }
        let mut index = 0;
        passage.messages.retain(|_| {
            let retain = keep.contains(&index);
            index += 1;
            retain
        });
        passage.truncated |= passage
            .messages
            .iter()
            .any(|message| message.offset > 0 || message.next_offset.is_some());
    }
    Ok(passages)
}
fn excerpt(message: &Chunk, text: &str, anchor: bool, requested: usize) -> PassageMessage {
    let offset = text.floor_char_boundary(requested.min(text.len()));
    let end = text.floor_char_boundary((offset + 4 * 1024).min(text.len()));
    PassageMessage {
        message_id: message.message_id.clone(),
        role: message.role,
        text: text[offset..end].into(),
        timestamp: message.timestamp,
        is_anchor: anchor,
        offset,
        next_offset: (end < text.len()).then_some(end),
        attachments: message.attachments.clone(),
    }
}
