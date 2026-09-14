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

use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use maka_runtime::artifact::content_digest;
use std::{
    collections::HashSet,
    io::{self, Read},
    path::Path,
};

const MAX_SOURCE_BYTES: u64 = 1024 * 1024;
const MAX_FILE_CHARS: usize = 6000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstructionFile {
    Agents,
    Claude,
    Gemini,
}

impl InstructionFile {
    pub fn name(self) -> &'static str {
        match self {
            Self::Agents => "AGENTS.md",
            Self::Claude => "CLAUDE.md",
            Self::Gemini => "GEMINI.md",
        }
    }
}

#[derive(Debug)]
pub struct WorkspaceInstruction {
    pub file: InstructionFile,
    pub text: String,
    pub truncated: bool,
}

/// Optional, read-only context. Unreadable/invalid sources are skipped, never
/// converted into tool grants. The caller joins this bounded blocking work.
pub fn read_instruction_files(root: &Path) -> Vec<WorkspaceInstruction> {
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    let Ok(directory) = Dir::open_ambient_dir(&root, ambient_authority()) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut instructions = Vec::new();
    for file in [
        InstructionFile::Agents,
        InstructionFile::Claude,
        InstructionFile::Gemini,
    ] {
        // Resolve absolute in-root aliases, then open through the directory
        // capability. A retargeted path cannot escape via an ambient reopen.
        let Ok(resolved) = root.join(file.name()).canonicalize() else {
            continue;
        };
        let Ok(relative) = resolved.strip_prefix(&root) else {
            continue;
        };
        let Ok(raw) = read_file(&directory, relative) else {
            continue;
        };
        let cleaned: String = raw
            .trim_matches(js_space)
            .chars()
            .filter(|c| !matches!(*c as u32, 0..=8 | 11 | 12 | 14..=31 | 127))
            .collect();
        if cleaned.is_empty() || !seen.insert(content_digest(cleaned.as_bytes())) {
            continue;
        }
        let mut chars = cleaned.chars();
        let text = chars.by_ref().take(MAX_FILE_CHARS).collect();
        instructions.push(WorkspaceInstruction {
            file,
            text,
            truncated: chars.next().is_some(),
        });
    }
    instructions
}

fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

fn read_file(directory: &Dir, relative: &Path) -> io::Result<String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = directory.open_with(relative, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return Err(io::Error::other(
            "instruction source is not a bounded regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(io::Error::other(
            "instruction source grew beyond the byte limit",
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
