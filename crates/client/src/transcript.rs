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

use crate::{Client, Error};
mod live;
use base64::{Engine, engine::general_purpose::STANDARD};
pub use live::LiveText;
use maka_protocol::transcript::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

const MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const BATCH_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct TranscriptRow {
    pub sequence: u64,
    pub value: Value,
}
#[derive(Debug)]
pub struct TranscriptBatch {
    /// Always chronological, including when fetched in the older direction.
    pub rows: Vec<TranscriptRow>,
    pub next_cursor: Option<String>,
    pub through_sequence: Option<u64>,
}

impl Client {
    /// Finish only the partial message at a page edge, not the entire history.
    /// The consumer decides when to fetch another page and how much to retain.
    pub async fn complete_transcript_page(
        &self,
        subscription: &str,
        mut page: SessionTranscriptPage,
    ) -> Result<TranscriptBatch, Error> {
        let direction = page.direction;
        let through_sequence = page.through_sequence;
        let session = page.session_id.clone();
        let mut assembly = Assembler::new(direction);
        for _ in 0..128 {
            if page.session_id != session
                || page.direction != direction
                || page.through_sequence != through_sequence
            {
                return Err("Transcript continuation identity changed".into());
            }
            assembly.accept(&page.fragments)?;
            let Some(remaining) = assembly.remaining() else {
                return Ok(TranscriptBatch {
                    rows: assembly.finish()?,
                    next_cursor: page.next_cursor,
                    through_sequence,
                });
            };
            let cursor = page
                .next_cursor
                .ok_or("Transcript ended inside a message")?;
            page = self
                .transcript_page(SessionTranscriptPageInput {
                    subscription_id: subscription.into(),
                    direction,
                    through_sequence,
                    cursor: Some(cursor),
                    anchor_sequence: None,
                    max_bytes: (remaining as u64).min(SESSION_TRANSCRIPT_PAGE_MAX_BYTES),
                })
                .await?;
        }
        Err("Transcript continuation exceeded page limit".into())
    }
}

struct Partial {
    sequence: u64,
    digest: Option<String>,
    bytes: Vec<u8>,
    edge: usize,
}
struct Assembler {
    direction: SessionTranscriptPageDirection,
    current: Option<Partial>,
    last: Option<u64>,
    rows: Vec<TranscriptRow>,
    bytes: usize,
}
impl Assembler {
    fn new(direction: SessionTranscriptPageDirection) -> Self {
        Self {
            direction,
            current: None,
            last: None,
            rows: Vec::new(),
            bytes: 0,
        }
    }
    fn remaining(&self) -> Option<usize> {
        self.current.as_ref().map(|row| match self.direction {
            SessionTranscriptPageDirection::Older => row.edge,
            SessionTranscriptPageDirection::Newer => row.bytes.len() - row.edge,
        })
    }
    fn accept(&mut self, fragments: &[SessionTranscriptFragment]) -> Result<(), Error> {
        for fragment in fragments {
            let total = usize::try_from(fragment.total_bytes)?;
            if total == 0 || total > MESSAGE_BYTES {
                return Err("Transcript message exceeds local byte limit".into());
            }
            let data = STANDARD.decode(&fragment.data)?;
            if data.is_empty() {
                return Err("Empty transcript fragment".into());
            }
            if self.current.is_none() {
                if self.bytes + total > BATCH_BYTES
                    || self.rows.len() >= SESSION_TRANSCRIPT_PAGE_MAX_MESSAGES
                {
                    return Err("Transcript batch exceeds local capacity".into());
                }
                if self.last.is_some_and(|previous| match self.direction {
                    SessionTranscriptPageDirection::Older => fragment.sequence >= previous,
                    SessionTranscriptPageDirection::Newer => fragment.sequence <= previous,
                }) {
                    return Err("Transcript message order changed".into());
                }
                self.last = Some(fragment.sequence);
                self.bytes += total;
                self.current = Some(Partial {
                    sequence: fragment.sequence,
                    digest: fragment.payload_digest.clone(),
                    bytes: vec![0; total],
                    edge: if self.direction == SessionTranscriptPageDirection::Older {
                        total
                    } else {
                        0
                    },
                });
            }
            let row = self.current.as_mut().expect("started");
            if row.sequence != fragment.sequence
                || row.bytes.len() != total
                || row.digest != fragment.payload_digest
            {
                return Err("Transcript fragment identity changed".into());
            }
            let offset = usize::try_from(fragment.byte_offset)?;
            let expected = match self.direction {
                SessionTranscriptPageDirection::Older => row.edge.checked_sub(data.len()),
                SessionTranscriptPageDirection::Newer => Some(row.edge),
            };
            let end = offset
                .checked_add(data.len())
                .ok_or("Transcript offset overflow")?;
            if expected != Some(offset) || end > total {
                return Err("Transcript fragment gap or overlap".into());
            }
            row.bytes[offset..end].copy_from_slice(&data);
            row.edge = if self.direction == SessionTranscriptPageDirection::Older {
                offset
            } else {
                end
            };
            if self.remaining() == Some(0) {
                let row = self.current.take().expect("complete");
                if row.digest.as_ref().is_some_and(|digest| {
                    *digest != format!("sha256:{:x}", Sha256::digest(&row.bytes))
                }) {
                    return Err("Transcript payload digest mismatch".into());
                }
                self.rows.push(TranscriptRow {
                    sequence: row.sequence,
                    value: serde_json::from_slice(&row.bytes)?,
                });
            }
        }
        Ok(())
    }
    fn finish(mut self) -> Result<Vec<TranscriptRow>, Error> {
        if self.current.is_some() {
            return Err("Incomplete transcript message".into());
        }
        if self.direction == SessionTranscriptPageDirection::Older {
            self.rows.reverse();
        }
        Ok(self.rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fragments() -> Vec<SessionTranscriptFragment> {
        let bytes = serde_json::to_vec(&json!({"text":"中文 🦀 é"})).unwrap();
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        bytes
            .chunks(3)
            .enumerate()
            .map(|(index, part)| SessionTranscriptFragment {
                sequence: 42,
                byte_offset: (index * 3) as u64,
                total_bytes: bytes.len() as u64,
                payload_digest: Some(digest.clone()),
                data: STANDARD.encode(part),
            })
            .collect()
    }
    #[test]
    fn fragments_reassemble_split_utf8_in_both_directions_and_fail_closed_on_corruption() {
        for direction in [
            SessionTranscriptPageDirection::Older,
            SessionTranscriptPageDirection::Newer,
        ] {
            let mut parts = fragments();
            if direction == SessionTranscriptPageDirection::Older {
                parts.reverse();
            }
            let mut assembly = Assembler::new(direction);
            for part in &parts {
                assembly.accept(std::slice::from_ref(part)).unwrap();
            }
            let rows = assembly.finish().unwrap();
            assert_eq!(rows[0].sequence, 42);
            assert_eq!(rows[0].value["text"], "中文 🦀 é");
            for defect in ["gap", "identity", "digest", "oversize", "truncated"] {
                let mut parts = parts.clone();
                match defect {
                    "gap" => parts[1].byte_offset += 1,
                    "identity" => parts[1].sequence += 1,
                    "digest" => {
                        for part in &mut parts {
                            part.payload_digest = Some(format!("sha256:{}", "0".repeat(64)));
                        }
                    }
                    "oversize" => parts[0].total_bytes = MESSAGE_BYTES as u64 + 1,
                    "truncated" => {
                        parts.pop();
                    }
                    _ => unreachable!(),
                }
                let mut assembly = Assembler::new(direction);
                assert!(
                    assembly
                        .accept(&parts)
                        .and_then(|_| assembly.finish())
                        .is_err(),
                    "{defect}"
                );
            }
        }
    }
}
