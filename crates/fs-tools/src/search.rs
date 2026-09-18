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

use crate::{ReadExecutor, failed};
use cap_std::fs::Dir;
use glob::MatchOptions;
use maka_runtime::tools::ToolError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

mod pattern;
use pattern::{Segment, compile};

pub const GLOB_NAME: &str = "Glob";
pub const GLOB_DESCRIPTION: &str = "Find paths matching a Unix glob (*, ?, **, character classes), relative to the search directory. Returns files and complete, with at most 200 paths; complete=false means some paths were not scanned. Wildcard matching is case-insensitive on macOS and Windows, case-sensitive on Linux. Hidden names require an explicit dot; recursive wildcards do not follow directory symlinks. cwd is limited to the Session's admitted filesystem roots.";

pub fn glob_schema() -> Value {
    json!({"type":"object","properties":{
        "pattern":{"type":"string","minLength":1},
        "cwd":{"type":"string","minLength":1}
    },"required":["pattern"],"additionalProperties":false})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    pattern: String,
    #[serde(default = "default_cwd")]
    cwd: String,
}
fn default_cwd() -> String {
    ".".into()
}

impl ReadExecutor {
    pub async fn glob(
        &self,
        input: Value,
        cancellation: CancellationToken,
    ) -> Result<Value, ToolError> {
        let input: Input = serde_json::from_value(input).map_err(|e| failed(e.to_string()))?;
        let segments = compile(&input.pattern)?;
        if input.cwd.is_empty() || input.cwd.contains("://") {
            return Err(failed("Glob cwd requires a filesystem directory"));
        }
        let authority = self.authority.clone();
        tokio::task::spawn_blocking(move || {
            let mut search = Search {
                cancellation,
                deadline: Instant::now() + Duration::from_secs(120),
                visited: 0,
                bytes: 0,
                files: BTreeSet::new(),
                complete: true,
            };
            search.check()?;
            let mut opened = None;
            let mut error = failed("Glob cwd is outside the admitted Read roots");
            for route in authority.routes(Path::new(&input.cwd))? {
                let root = route.root;
                let path = route.relative;
                match root.open_dir(if path.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    &path
                }) {
                    Ok(dir) => {
                        opened = Some(dir);
                        break;
                    }
                    Err(reason) => error = failed(format!("Glob cwd: {reason}")),
                }
            }
            let directory = opened.ok_or(error)?;
            if !segments.is_empty() {
                search.walk(&directory, Path::new(""), &segments, 0)?;
            }
            search.check()?;
            Ok(json!({"files":search.files,"complete":search.complete}))
        })
        .await
        .map_err(|e| failed(format!("Glob worker failed: {e}")))?
    }
}

struct Search {
    cancellation: CancellationToken,
    deadline: Instant,
    visited: usize,
    bytes: usize,
    files: BTreeSet<String>,
    complete: bool,
}

impl Search {
    fn check(&self) -> Result<(), ToolError> {
        if self.cancellation.is_cancelled() {
            return Err(failed("Glob cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(failed("Glob search timed out"));
        }
        Ok(())
    }

    fn record(&mut self, relative: &Path) -> Result<(), ToolError> {
        let path = if relative.as_os_str().is_empty() {
            "."
        } else {
            relative
                .to_str()
                .ok_or_else(|| failed("Glob path is not UTF-8"))?
        };
        if !self.files.contains(path) {
            self.bytes += serde_json::to_string(path)
                .map_err(|e| failed(e.to_string()))?
                .len()
                + 1;
            if self.bytes > 1024 * 1024 - 32 {
                return Err(failed("Glob result exceeds byte limit"));
            }
            self.files.insert(path.to_owned());
        }
        Ok(())
    }

    fn walk(
        &mut self,
        root: &Dir,
        relative: &Path,
        segments: &[Segment],
        depth: usize,
    ) -> Result<(), ToolError> {
        self.check()?;
        if self.files.len() >= 200 {
            self.complete = false;
            return Ok(());
        }
        let Some((segment, rest)) = segments.split_first() else {
            return self.record(relative);
        };
        if matches!(segment, Segment::Directory) {
            return self.record(relative);
        }
        if let Segment::Literal(name) = segment {
            let child = relative.join(name);
            let metadata = match root.symlink_metadata(&child) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(failed(format!("Glob path: {error}"))),
            };
            if rest.is_empty() {
                return self.record(&child);
            }
            if metadata.is_dir() || metadata.is_symlink() {
                return self.walk(root, &child, rest, depth + 1);
            }
            return Ok(());
        }
        if depth > 128 {
            return Err(failed("Glob search exceeds directory depth limit"));
        }
        let recursive = matches!(segment, Segment::Recursive);
        if recursive {
            self.walk(root, relative, rest, depth)?;
        }
        if self.files.len() >= 200 {
            self.complete = false;
            return Ok(());
        }
        let path = if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        };
        let entries = root
            .read_dir(path)
            .map_err(|e| failed(format!("Glob directory: {e}")))?;
        let mut children = Vec::new();
        for entry in entries {
            self.check()?;
            self.visited += 1;
            if self.visited > 100_000 {
                return Err(failed("Glob search exceeds directory entry limit"));
            }
            let entry = entry.map_err(|e| failed(format!("Glob directory entry: {e}")))?;
            children.push((
                entry.file_name(),
                entry
                    .file_type()
                    .map_err(|e| failed(format!("Glob file type: {e}")))?,
            ));
        }
        children.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, kind) in children {
            self.check()?;
            if self.files.len() >= 200 {
                self.complete = false;
                break;
            }
            let name = name
                .to_str()
                .ok_or_else(|| failed("Glob path is not UTF-8"))?;
            let matched = match segment {
                Segment::Recursive => !name.starts_with('.'),
                Segment::Directory => unreachable!("directory suffix handled before walking"),
                Segment::Literal(_) => unreachable!("literal path handled before walking"),
                Segment::Match(pattern) => pattern.matches_with(
                    name,
                    MatchOptions {
                        case_sensitive: cfg!(target_os = "linux"),
                        require_literal_separator: true,
                        require_literal_leading_dot: true,
                    },
                ),
            };
            if !matched {
                continue;
            }
            let child: PathBuf = relative.join(name);
            if rest.is_empty() {
                self.record(&child)?;
            }
            if kind.is_dir() && (recursive || !rest.is_empty()) {
                self.walk(
                    root,
                    &child,
                    if recursive { segments } else { rest },
                    depth + 1,
                )?;
            }
        }
        Ok(())
    }
}
