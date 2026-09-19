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

use super::{Code, OperationError, failure};
use maka_runtime::input::MessageInput;

pub(super) fn content(
    mut content: MessageInput,
    text: &str,
) -> Result<MessageInput, OperationError> {
    let visible: Vec<u16> = text.encode_utf16().collect();
    if let Some(references) = &mut content.inline_references {
        references.retain_mut(|reference| {
            let token: Vec<_> = reference.value.encode_utf16().collect();
            let start = usize::try_from(reference.start).unwrap_or(usize::MAX);
            if start
                .checked_add(token.len())
                .is_some_and(|end| visible.get(start..end) == Some(token.as_slice()))
            {
                return true;
            }
            if token.is_empty() {
                return false;
            }
            let mut occurrences = visible
                .windows(token.len())
                .enumerate()
                .filter_map(|(i, window)| (window == token).then_some(i));
            let Some(first) = occurrences.next() else {
                return false;
            };
            // Match the source's non-overlapping second indexOf search.
            if occurrences.any(|next| next >= first + token.len()) {
                return false;
            }
            reference.start = first as u64;
            true
        });
        references.sort_by(|a, b| {
            a.start.cmp(&b.start).then_with(|| {
                b.value
                    .encode_utf16()
                    .count()
                    .cmp(&a.value.encode_utf16().count())
            })
        });
        let mut end = 0;
        references.retain(|reference| {
            if reference.start < end {
                return false;
            }
            end = reference.start + reference.value.encode_utf16().count() as u64;
            true
        });
    }
    content.text = text.into();
    content.display_text = None;
    // Apply the same canonical wire bounds before any durable update.
    let mut wire = maka_protocol::turn::MessageContent::from(content);
    wire.validate_admission(false)
        .map_err(|e| failure(Code::OperationConflict, &e.to_string()))?;
    Ok(wire.into())
}
