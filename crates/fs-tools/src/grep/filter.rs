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

use super::io_error;
use crate::failed;
use cap_std::fs::Dir;
use ignore::{
    gitignore::{Gitignore, GitignoreBuilder},
    overrides::{Override, OverrideBuilder},
};
use maka_runtime::tools::ToolError;
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

struct Layer {
    rules: [Gitignore; 4],
    in_git: bool,
}
pub(super) struct Filters {
    layers: Vec<Layer>,
    overrides: Override,
    loaded_bytes: usize,
}
impl Filters {
    pub(super) fn new(glob: Option<&str>, cwd: &Path) -> Result<Self, ToolError> {
        let mut builder = OverrideBuilder::new(cwd);
        if let Some(glob) = glob.filter(|g| !g.is_empty()) {
            builder
                .add(glob)
                .map_err(|e| failed(format!("Grep glob: {e}")))?;
        }
        Ok(Self {
            layers: Vec::new(),
            overrides: builder.build().map_err(|e| failed(e.to_string()))?,
            loaded_bytes: 0,
        })
    }
    pub(super) fn push(&mut self, root: &Dir, path: &Path) -> Result<(), ToolError> {
        let in_git = self.layers.last().is_some_and(|layer| layer.in_git)
            || root.metadata(path.join(".git")).is_ok();
        let rules = [
            self.rules(root, path, ".rgignore")?,
            self.rules(root, path, ".ignore")?,
            if in_git {
                self.rules(root, path, ".gitignore")?
            } else {
                Gitignore::empty()
            },
            if in_git {
                self.rules(root, path, ".git/info/exclude")?
            } else {
                Gitignore::empty()
            },
        ];
        self.layers.push(Layer { rules, in_git });
        Ok(())
    }
    pub(super) fn pop(&mut self) {
        self.layers.pop();
    }
    pub(super) fn excludes(
        &self,
        path: &Path,
        display: &Path,
        directory: bool,
        os_hidden: bool,
    ) -> bool {
        let override_match = self.overrides.matched(display, directory);
        if !override_match.is_none() {
            return override_match.is_ignore();
        }
        for priority in 0..4 {
            for layer in self.layers.iter().rev() {
                let matched = layer.rules[priority].matched_path_or_any_parents(path, directory);
                if !matched.is_none() {
                    return matched.is_ignore();
                }
            }
        }
        os_hidden || hidden(path)
    }
    fn rules(&mut self, root: &Dir, path: &Path, name: &str) -> Result<Gitignore, ToolError> {
        let source: PathBuf = path.join(name);
        let file = match super::open(root, &source) {
            Ok(file) => file,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(Gitignore::empty());
            }
            Err(error) => return Err(io_error(error)),
        };
        let metadata = file.metadata().map_err(io_error)?;
        if !metadata.is_file() || metadata.len() > 256 * 1024 {
            return Err(failed(
                "Grep ignore source must be a regular file within 256 KiB",
            ));
        }
        let mut bytes = Vec::new();
        file.take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        self.loaded_bytes += bytes.len();
        if bytes.len() > 256 * 1024 || self.loaded_bytes > 16 * 1024 * 1024 {
            return Err(failed("Grep ignore sources exceed byte limit"));
        }
        let mut builder = GitignoreBuilder::new(path);
        for line in String::from_utf8_lossy(&bytes).lines() {
            builder
                .add_line(Some(source.clone()), line)
                .map_err(|e| failed(format!("Grep ignore rule: {e}")))?;
        }
        builder
            .build()
            .map_err(|e| failed(format!("Grep ignore rules: {e}")))
    }
}
fn hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
}
