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

use maka_plugins::filesystem::ReadError;
use maka_runtime::import::{MAX_IMPORT_BYTES, MAX_IMPORT_RECORDS, Record, Source};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Read(#[from] ReadError),
    #[error(transparent)]
    Database(#[from] maka_plugins::filesystem::database::Error),
    #[error("invalid source record at line {line}: {source}")]
    Decode {
        line: u64,
        source: serde_json::Error,
    },
    #[error("source limit exceeded: {kind} (maximum {max})")]
    Limit { kind: &'static str, max: u64 },
    #[error("invalid source: {0}")]
    Invalid(&'static str),
}

/// Evidence for one fixed prefix, including any incomplete final write. Pinning
/// alone does not freeze in-place writes; multi-pass formats compare this digest.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub bytes: u64,
    pub sha256: String,
    pub incomplete_tail: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub source: Source,
    /// Source claims only; never a workspace or model authorization.
    pub cwd: Option<String>,
    pub title: String,
    pub records: Vec<Record>,
    pub fingerprint: Fingerprint,
}

#[derive(Default)]
pub(crate) struct Records {
    records: Vec<Record>,
    bytes: u64,
}
impl Records {
    pub fn push(&mut self, record: Record) -> Result<(), Error> {
        if self.records.len() as u64 >= MAX_IMPORT_RECORDS {
            return Err(Error::Limit {
                kind: "records",
                max: MAX_IMPORT_RECORDS,
            });
        }
        let bytes = retained_size(&record)?;
        if bytes > MAX_IMPORT_BYTES - self.bytes {
            return Err(Error::Limit {
                kind: "converted_bytes",
                max: MAX_IMPORT_BYTES,
            });
        }
        self.bytes += bytes;
        record.validate().map_err(Error::Invalid)?;
        self.records.push(record);
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<Record>, Error> {
        if !self.records.iter().any(Record::is_conversation) {
            return Err(Error::Invalid("source contains no readable conversation"));
        }
        Ok(self.records)
    }
    pub fn retain(&mut self, keep: impl FnMut(&Record) -> bool) -> Result<(), Error> {
        self.records.retain(keep);
        self.bytes = 0;
        for record in &self.records {
            self.bytes += retained_size(record)?;
        }
        Ok(())
    }
}

pub(crate) fn retained_size(value: &impl Serialize) -> Result<u64, Error> {
    struct Counter {
        bytes: u64,
        exceeded: bool,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() as u64 > MAX_IMPORT_BYTES - self.bytes {
                self.exceeded = true;
                return Err(std::io::Error::other("converted byte limit"));
            }
            self.bytes += bytes.len() as u64;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter {
        bytes: 0,
        exceeded: false,
    };
    match serde_json::to_writer(&mut counter, value) {
        Ok(()) => Ok(counter.bytes),
        Err(_) if counter.exceeded => Err(Error::Limit {
            kind: "converted_bytes",
            max: MAX_IMPORT_BYTES,
        }),
        Err(_) => Err(Error::Invalid("invalid converted record")),
    }
}

pub(crate) fn identity(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        Err(Error::Invalid("invalid source identity"))
    } else {
        Ok(())
    }
}

pub(crate) fn source_cwd(value: Option<String>) -> Option<String> {
    value.filter(|path| {
        !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control)
    })
}

pub(crate) fn title(text: &str) -> String {
    let mut output = String::new();
    let mut remaining = 160;
    for word in text.split_whitespace() {
        if !output.is_empty() {
            if remaining <= 1 {
                break;
            }
            output.push(' ');
            remaining -= 1;
        }
        for character in word.chars().take(remaining) {
            output.push(character);
            remaining -= 1;
        }
        if remaining == 0 {
            break;
        }
    }
    output
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum Timestamp {
    Number(f64),
    Text(String),
}
impl Timestamp {
    pub fn millis(&self) -> Option<u64> {
        self.convert(false)
    }
    pub fn codex_millis(&self) -> Option<u64> {
        self.convert(true)
    }
    fn convert(&self, codex: bool) -> Option<u64> {
        let number = match self {
            Self::Number(value) => Some(*value),
            Self::Text(value) => value.parse().ok(),
        };
        let millis = match number {
            Some(value) if value.is_finite() && value >= 0.0 => {
                if codex && value < 1_000_000_000_000.0 {
                    value * 1000.0
                } else {
                    value
                }
            }
            Some(_) => return None,
            None => {
                let Self::Text(value) = self else { return None };
                chrono::DateTime::parse_from_rfc3339(value)
                    .ok()?
                    .timestamp_millis() as f64
            }
        };
        (0.0..=9_007_199_254_740_991.0)
            .contains(&millis)
            .then_some(millis as u64)
    }
}
