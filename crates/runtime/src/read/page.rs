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

use super::{MAX_PAGE_CHARS, PREFIX, ReadError, ReadInput, ReadRequest, Start, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{cell::OnceCell, num::NonZeroUsize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadPage {
    pub content: String,
    pub offset: usize,
    pub returned_lines: usize,
    pub total_lines: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial_line: bool,
    pub next: Option<ReadInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<super::ReadMetadata>,
}

#[derive(Clone, Copy, Default)]
struct Position {
    byte: usize,
    utf16: usize,
}

impl ReadRequest {
    pub fn page(&self, text: &str) -> Result<ReadPage, ReadError> {
        self.page_with_budget(text, MAX_PAGE_CHARS)
    }

    /// Budget is the serialized page's UTF-16 length, including continuation.
    /// Callers adding execution metadata reserve its serialized size here.
    pub fn page_with_budget(&self, text: &str, budget: usize) -> Result<ReadPage, ReadError> {
        let digest = OnceCell::new();
        let requested_position = match &self.start {
            Start::Line(_) => None,
            Start::Continuation {
                position,
                digest: expected,
            } => {
                let actual = digest.get_or_init(|| content_digest(text));
                if actual != expected {
                    return Err(ReadError::ContentChanged);
                }
                Some(*position)
            }
        };
        let mut lines = vec![Position::default()];
        let mut position = Position::default();
        let mut resumed = None;
        for (byte, character) in text.char_indices() {
            if requested_position == Some(position.utf16) {
                resumed = Some(position);
            }
            position = Position {
                byte: byte + character.len_utf8(),
                utf16: position.utf16 + character.len_utf16(),
            };
            if character == '\n' {
                lines.push(position);
            }
        }
        if requested_position == Some(position.utf16) {
            resumed = Some(position);
        }
        let line_at = |byte| lines.partition_point(|line| line.byte <= byte) - 1;
        let (offset, start) = match self.start {
            Start::Line(offset) => (offset, lines.get(offset).copied().unwrap_or(position)),
            Start::Continuation { .. } => {
                let start = resumed.ok_or(ReadError::InvalidPosition)?;
                (line_at(start.byte), start)
            }
        };
        let last_line = offset
            .saturating_add(self.limit.map_or(lines.len(), NonZeroUsize::get))
            .min(lines.len());
        let end = lines.get(last_line).map_or(position, |line| Position {
            byte: line.byte - 1,
            utf16: line.utf16 - 1,
        });
        let make_page = |stop: Position| {
            let complete = stop.byte >= end.byte;
            let next_line = offset.max(line_at(stop.byte));
            let boundary = lines
                .get(next_line + 1)
                .is_some_and(|line| stop.byte == line.byte - 1);
            let partial = !complete && !boundary;
            let returned_lines = if offset >= lines.len() {
                0
            } else if complete {
                last_line - offset
            } else {
                next_line - offset + usize::from(!partial)
            };
            ReadPage {
                metadata: None,
                content: text[start.byte..stop.byte.max(start.byte)].into(),
                offset,
                returned_lines,
                total_lines: lines.len(),
                partial_line: partial || (offset < lines.len() && start.byte > lines[offset].byte),
                next: (!complete).then(|| ReadInput {
                    path: format!(
                        "{PREFIX}{}?at={}&sha={}",
                        URL_SAFE_NO_PAD.encode(&self.path),
                        stop.utf16 + usize::from(!partial),
                        digest.get_or_init(|| content_digest(text))
                    ),
                    offset: None,
                    limit: self.limit.map(|limit| {
                        NonZeroUsize::new(limit.get() - returned_lines)
                            .expect("incomplete line range")
                    }),
                }),
            }
        };
        let fits = |page: &ReadPage| {
            serde_json::to_string(page)
                .expect("ReadPage contains only JSON fields")
                .encode_utf16()
                .count()
                <= budget
        };
        if end.utf16.saturating_sub(start.utf16) <= budget {
            let page = make_page(end);
            if fits(&page) {
                return Ok(page);
            }
        }
        // Only retain candidate boundaries for this bounded page, not every
        // character in a potentially large source. Rust slices remain UTF-8 safe.
        let mut candidates = vec![start];
        let mut utf16 = start.utf16;
        for (relative, character) in text[start.byte..end.byte].char_indices() {
            utf16 += character.len_utf16();
            if utf16 - start.utf16 > budget {
                break;
            }
            candidates.push(Position {
                byte: start.byte + relative + character.len_utf8(),
                utf16,
            });
        }
        let (mut low, mut high) = (0, candidates.len() - 1);
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if fits(&make_page(candidates[middle])) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        if low == 0 {
            return Err(ReadError::AddressTooLong);
        }
        let mut stop = candidates[low];
        if let Some(relative) = text[start.byte..stop.byte]
            .rfind('\n')
            .filter(|relative| *relative > 0 && text.as_bytes().get(stop.byte) != Some(&b'\n'))
        {
            stop = Position {
                byte: start.byte + relative,
                utf16: start.utf16
                    + text[start.byte..start.byte + relative]
                        .encode_utf16()
                        .count(),
            };
        }
        let page = make_page(stop);
        if !fits(&page) {
            return Err(ReadError::AddressTooLong);
        }
        Ok(page)
    }
}

fn content_digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))[..32].into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bounded_pages_preserve_line_ranges_unicode_and_reject_changed_content() {
        for text in [
            String::new(),
            "\n\n".into(),
            "一\r\n二\n".into(),
            format!(
                "\n{}\n{}\nend\n",
                "中😀\"\\\t".repeat(2_000),
                "short\n".repeat(800)
            ),
        ] {
            for (offset, limit) in [(0, None), (1, Some(2)), (999_999, Some(3))] {
                let mut input = ReadInput {
                    path: "maka://runtime/attachments/报告".into(),
                    offset: Some(offset),
                    limit: limit.and_then(NonZeroUsize::new),
                };
                let expected = text
                    .split('\n')
                    .skip(offset)
                    .take(limit.unwrap_or(usize::MAX))
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut actual = String::new();
                let mut consumed = text
                    .split_inclusive('\n')
                    .take(offset)
                    .map(|line| line.encode_utf16().count())
                    .sum::<usize>();
                let mut count = 0;
                loop {
                    let page = input
                        .resolve()
                        .unwrap()
                        .page_with_budget(&text, 512)
                        .unwrap();
                    assert!(serde_json::to_string(&page).unwrap().encode_utf16().count() <= 512);
                    actual.push_str(&page.content);
                    consumed += page.content.encode_utf16().count();
                    count += 1;
                    assert!(count < 1_000, "every continuation must progress");
                    let Some(next) = page.next else { break };
                    assert!(!page.content.is_empty());
                    let Start::Continuation { position, .. } = next.resolve().unwrap().start else {
                        panic!("expected continuation")
                    };
                    assert!((consumed..=consumed + 1).contains(&position));
                    if position > consumed {
                        actual.push('\n');
                        consumed += 1;
                    }
                    assert_eq!(
                        next.resolve().unwrap().page(&(text.clone() + "changed")),
                        Err(ReadError::ContentChanged)
                    );
                    input = next;
                }
                assert!(
                    actual == expected,
                    "line range offset={offset} limit={limit:?} did not reconstruct the source"
                );
            }
        }
        let text = "😀".repeat(8_000);
        let input: ReadInput = serde_json::from_value(json!({"path":"file"})).unwrap();
        let page = input.resolve().unwrap().page(&text).unwrap();
        assert!(page.partial_line);
        let mut request = page.next.unwrap().resolve().unwrap();
        for invalid in [1, text.encode_utf16().count() + 1] {
            request.start = Start::Continuation {
                position: invalid,
                digest: content_digest(&text),
            };
            assert_eq!(request.page(&text), Err(ReadError::InvalidPosition));
        }
        let input: ReadInput =
            serde_json::from_value(json!({"path":"a".repeat(MAX_PAGE_CHARS)})).unwrap();
        assert_eq!(
            input.resolve().unwrap().page(&text),
            Err(ReadError::AddressTooLong)
        );
    }
}
