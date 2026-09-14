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

use crate::failed;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};
use maka_runtime::tools::ToolError;
use std::{io, path::Path, time::Instant};
use tokio_util::sync::CancellationToken;

pub(super) struct Regex(grep_regex::RegexMatcher);
impl Regex {
    pub(super) fn new(pattern: &str) -> Result<Self, ToolError> {
        grep_regex::RegexMatcherBuilder::new()
            .multi_line(true)
            .line_terminator(Some(b'\n'))
            .build(pattern)
            .map(Self)
            .map_err(|e| failed(format!("Grep regex: {e}")))
    }
    pub(super) fn search(
        &self,
        bytes: &[u8],
        label: Option<&Path>,
        cancellation: &CancellationToken,
        deadline: Instant,
        output: &mut Vec<String>,
        output_bytes: &mut usize,
    ) -> Result<bool, ToolError> {
        let mut sink = Lines {
            cancellation,
            deadline,
            label,
            output,
            output_bytes,
            count: 0,
            complete: true,
        };
        SearcherBuilder::new()
            .line_number(true)
            .bom_sniffing(false) // Decoded and bounded before binary detection.
            .build()
            .search_slice(&self.0, bytes, &mut sink)
            .map_err(|e| failed(e.to_string()))?;
        Ok(sink.complete)
    }
}
struct Lines<'a> {
    cancellation: &'a CancellationToken,
    deadline: Instant,
    label: Option<&'a Path>,
    output: &'a mut Vec<String>,
    output_bytes: &'a mut usize,
    count: usize,
    complete: bool,
}
impl Sink for Lines<'_> {
    type Error = io::Error;
    fn matched(&mut self, _: &Searcher, matched: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::other("Grep cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(io::Error::other("Grep timed out"));
        }
        // One lookahead match distinguishes an exact cap from omitted results.
        if self.count == 50 || self.output.len() == 200 {
            self.complete = false;
            return Ok(false);
        }
        let line = matched
            .bytes()
            .strip_suffix(b"\n")
            .unwrap_or(matched.bytes());
        let line = String::from_utf8_lossy(line);
        let number = matched
            .line_number()
            .ok_or_else(|| io::Error::other("Grep line number missing"))?;
        let rendered = match self.label {
            Some(label) => format!(
                "{}:{number}:{line}",
                label
                    .to_str()
                    .ok_or_else(|| io::Error::other("Grep path is not UTF-8"))?
            ),
            None => format!("{number}:{line}"),
        };
        *self.output_bytes += serde_json::to_string(&rendered)?.len() + 1;
        if *self.output_bytes > 1024 * 1024 - 32 {
            return Err(io::Error::other("Grep result exceeds byte limit"));
        }
        self.output.push(rendered);
        self.count += 1;
        Ok(true)
    }
}
