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

use crate::{Error, Fingerprint};
use maka_plugins::filesystem::{PinnedReader, ReadRange};
use maka_runtime::import::MAX_IMPORT_BYTES;
use serde::Deserialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

pub(crate) const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;
const MAX_LINES: u64 = 1_000_000;
const MAX_STRUCTURE: u64 = 65_536;

pub(crate) fn decode<'a, T: Deserialize<'a>>(value: &'a RawValue, line: u64) -> Result<T, Error> {
    if value.get().len() as u64 > MAX_IMPORT_BYTES {
        return Err(Error::Limit {
            kind: "converted_bytes",
            max: MAX_IMPORT_BYTES,
        });
    }
    complexity(value.get().as_bytes())?;
    serde_json::from_str(value.get()).map_err(|source| Error::Decode { line, source })
}

// RawValue already validated the syntax. Bound object/array expansion before
// any typed decoder allocates a Value or serde's internally-tagged buffer.
fn complexity(bytes: &[u8]) -> Result<(), Error> {
    let mut quoted = false;
    let mut escaped = false;
    let mut structure = 0;
    for byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else if *byte == b'"' {
            quoted = true;
        } else if matches!(byte, b'[' | b']' | b'{' | b'}' | b',' | b':') {
            structure += 1;
            if structure > MAX_STRUCTURE {
                return Err(Error::Limit {
                    kind: "record_complexity",
                    max: MAX_STRUCTURE,
                });
            }
        }
    }
    Ok(())
}

pub(crate) struct Line<'a> {
    pub number: u64,
    pub json: &'a RawValue,
}
impl Line<'_> {
    pub fn decode<'a, T: Deserialize<'a>>(&'a self) -> Result<T, Error> {
        serde_json::from_str(self.json.get()).map_err(|source| Error::Decode {
            line: self.number,
            source,
        })
    }
}

/// All formats get strict interior parsing, bounded buffering and a source digest.
/// Only a syntactically unfinished final write is ignored, never a corrupt row.
pub(crate) fn scan(
    reader: &mut PinnedReader<'_>,
    length: u64,
    mut accept: impl FnMut(Line<'_>) -> Result<(), Error>,
) -> Result<Fingerprint, Error> {
    if length > MAX_BYTES {
        return Err(Error::Limit {
            kind: "source_bytes",
            max: MAX_BYTES,
        });
    }
    let mut digest = Sha256::new();
    let mut pending = Vec::new();
    let mut offset = 0;
    let mut number = 0;
    loop {
        let page = reader.read(ReadRange {
            offset,
            limit: 64 * 1024,
        })?;
        digest.update(&page.bytes);
        for segment in page.bytes.split_inclusive(|byte| *byte == b'\n') {
            if segment.len() > MAX_LINE_BYTES - pending.len() {
                return Err(Error::Limit {
                    kind: "record_bytes",
                    max: MAX_LINE_BYTES as u64,
                });
            }
            pending.extend_from_slice(segment);
            if segment.last() == Some(&b'\n') {
                next(&mut number)?;
                parse(&pending, number, false, &mut accept)?;
                pending.clear();
            }
        }
        match page.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    let incomplete_tail = if pending.is_empty() {
        false
    } else {
        next(&mut number)?;
        parse(&pending, number, true, &mut accept)?
    };
    Ok(Fingerprint {
        bytes: length,
        sha256: format!("{:x}", digest.finalize()),
        incomplete_tail,
    })
}
fn next(number: &mut u64) -> Result<(), Error> {
    *number += 1;
    if *number > MAX_LINES {
        return Err(Error::Limit {
            kind: "source_records",
            max: MAX_LINES,
        });
    }
    Ok(())
}
fn parse(
    bytes: &[u8],
    number: u64,
    last: bool,
    accept: &mut impl FnMut(Line<'_>) -> Result<(), Error>,
) -> Result<bool, Error> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(false);
    }
    match serde_json::from_slice::<&RawValue>(bytes) {
        Ok(json) => {
            complexity(bytes)?;
            accept(Line { number, json })?;
            Ok(false)
        }
        Err(error) if last && error.is_eof() => Ok(true),
        Err(source) => Err(Error::Decode {
            line: number,
            source,
        }),
    }
}
