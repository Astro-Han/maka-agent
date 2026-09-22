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

use crate::scoped::Directory as Dir;
use crate::{ReadExecutor, failed};
use cap_std::fs::{File, OpenOptions, OpenOptionsExt};
use maka_runtime::tools::ToolError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

mod filter;
mod matches;

pub const GREP_NAME: &str = "Grep";
pub const GREP_DESCRIPTION: &str = "Search file contents with a ripgrep-compatible regular expression. Optional path selects a file or directory; glob filters files. Returns matches and complete. At most 200 matching lines, 50 per file; complete=false means limits left matches or paths unscanned, so narrow the pattern or path before treating results as exhaustive. Hidden, binary and ignored files are excluded during directory traversal. No fuzzy fallback.";

pub fn grep_schema() -> Value {
    schemars::schema_for!(Input).into()
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    #[schemars(length(max = 8192))]
    pattern: String,
    #[serde(default = "default_path")]
    path: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "String", length(max = 4096))]
    glob: Option<String>,
}
fn default_path() -> String {
    ".".into()
}
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}

impl ReadExecutor {
    pub async fn grep(
        &self,
        input: Value,
        cancellation: CancellationToken,
    ) -> Result<Value, ToolError> {
        let input: Input = serde_json::from_value(input).map_err(|e| failed(e.to_string()))?;
        if input.pattern.len() > 8192
            || input.path.contains("://")
            || input.glob.as_ref().is_some_and(|g| g.len() > 4096)
        {
            return Err(failed("Grep requires bounded filesystem search arguments"));
        }
        let matcher = matches::Regex::new(&input.pattern)?;
        let authority = self.authority.clone();
        tokio::task::spawn_blocking(move || {
            let mut search = Search {
                matcher,
                cancellation,
                deadline: Instant::now() + Duration::from_secs(120),
                visited: 0,
                source_bytes: 0,
                output_bytes: 0,
                output: Vec::new(),
                complete: true,
            };
            search.check()?;
            let requested = if input.path.is_empty() {
                "."
            } else {
                &input.path
            };
            let mut selected = None;
            let mut error = failed("Grep path is outside the admitted Read roots");
            for route in authority.routes(Path::new(requested))? {
                let relative = if route.relative.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    route.relative
                };
                match open(&route.root, &relative) {
                    Ok(file) => {
                        selected = Some((route.root, route.root_path, relative, file));
                        break;
                    }
                    Err(reason) => error = io_error(reason),
                }
            }
            let (root, root_path, relative, file) = selected.ok_or(error)?;
            let metadata = file.metadata().map_err(io_error)?;
            if metadata.is_file() {
                search.file(file, None)?;
            } else if metadata.is_dir() {
                let target = root.canonicalize(&relative).map_err(io_error)?;
                let display: PathBuf = root_path.join(&target).components().collect();
                let display = dunce::simplified(&display);
                let mut filters = filter::Filters::new(
                    input.glob.as_deref(),
                    dunce::simplified(authority.cwd()),
                )?;
                // Read ancestor rules only through the already admitted root.
                for ancestor in target
                    .ancestors()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    filters.push(&root, ancestor)?;
                }
                search.walk(&root, &target, &target, display, &mut filters, 0)?;
            } else {
                return Err(failed("Grep supports only regular files and directories"));
            }
            search.check()?;
            Ok(json!({"matches":search.output,"complete":search.complete}))
        })
        .await
        .map_err(|e| failed(format!("Grep worker failed: {e}")))?
    }
}

struct Search {
    matcher: matches::Regex,
    cancellation: CancellationToken,
    deadline: Instant,
    visited: usize,
    source_bytes: usize,
    output_bytes: usize,
    output: Vec<String>,
    complete: bool,
}
impl Search {
    fn check(&self) -> Result<(), ToolError> {
        if self.cancellation.is_cancelled() {
            return Err(failed("Grep cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(failed("Grep timed out"));
        }
        Ok(())
    }
    fn file(&mut self, file: File, label: Option<&Path>) -> Result<(), ToolError> {
        self.check()?;
        let metadata = file.metadata().map_err(io_error)?;
        if !metadata.is_file() {
            return Err(failed("Grep target changed to a nonregular file"));
        }
        if metadata.len() > 10 * 1024 * 1024 {
            return Err(failed("Grep source exceeds 10 MiB per-file limit"));
        }
        let mut bytes = Vec::new();
        let mut file = file.take(10 * 1024 * 1024 + 1);
        let mut chunk = [0; 8192];
        loop {
            self.check()?;
            let count = file.read(&mut chunk).map_err(io_error)?;
            if count == 0 {
                break;
            }
            self.source_bytes += count;
            if self.source_bytes > 256 * 1024 * 1024 {
                return Err(failed("Grep search exceeds total byte limit"));
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.len() > 10 * 1024 * 1024 {
                return Err(failed("Grep source grew beyond byte limit"));
            }
        }
        let decoded = encoding_rs::Encoding::for_bom(&bytes)
            .map(|(encoding, _)| encoding.decode_with_bom_removal(&bytes).0);
        let bytes = decoded
            .as_ref()
            .map_or(bytes.as_slice(), |text| text.as_bytes());
        if bytes.len() > 10 * 1024 * 1024 {
            return Err(failed("Grep decoded source exceeds 10 MiB per-file limit"));
        }
        if bytes.contains(&0) {
            return Ok(());
        }
        self.complete &= self.matcher.search(
            bytes,
            label,
            &self.cancellation,
            self.deadline,
            &mut self.output,
            &mut self.output_bytes,
        )?;
        self.check()
    }
    fn walk(
        &mut self,
        root: &Dir,
        path: &Path,
        target: &Path,
        display: &Path,
        filters: &mut filter::Filters,
        depth: usize,
    ) -> Result<(), ToolError> {
        self.check()?;
        if depth > 128 {
            return Err(failed("Grep directory depth limit exceeded"));
        }
        filters.push(root, path)?;
        let directory = if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            path
        };
        let mut children = Vec::new();
        for entry in root.read_dir(directory).map_err(io_error)? {
            self.check()?;
            self.visited += 1;
            if self.visited > 100_000 {
                return Err(failed("Grep directory entry limit exceeded"));
            }
            let entry = entry.map_err(io_error)?;
            #[cfg(unix)]
            let hidden = false;
            #[cfg(windows)]
            let hidden = {
                use cap_std::fs::MetadataExt;
                entry.metadata().map_err(io_error)?.file_attributes()
                    & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN
                    != 0
            };
            children.push((
                entry.file_name(),
                entry.file_type().map_err(io_error)?,
                hidden,
            ));
        }
        children.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, kind, hidden) in children {
            self.check()?;
            if !kind.is_dir() && !kind.is_file() {
                continue;
            }
            let child = path.join(&name);
            if let Err(error) = root.check_read(&child) {
                if error.kind() == std::io::ErrorKind::PermissionDenied {
                    continue;
                }
                return Err(io_error(error));
            }
            let label = display.join(
                child
                    .strip_prefix(target)
                    .map_err(|e| failed(e.to_string()))?,
            );
            if filters.excludes(&child, &label, kind.is_dir(), hidden) {
                continue;
            }
            if self.output.len() >= 200 {
                // We did not search this eligible path; do not claim exhaustive matches.
                self.complete = false;
                break;
            }
            if kind.is_dir() {
                self.walk(root, &child, target, display, filters, depth + 1)?;
            } else {
                self.file(open(root, &child).map_err(io_error)?, Some(&label))?;
            }
        }
        filters.pop();
        Ok(())
    }
}

fn open(root: &Dir, path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    #[cfg(windows)]
    options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS);
    root.open_with(path, &options)
}
fn io_error(e: std::io::Error) -> ToolError {
    ToolError::Io {
        kind: e.kind(),
        message: format!("Grep filesystem error: {e}"),
    }
}
