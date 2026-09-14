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

use super::{MARKER_FILE, invalid, options, read_bounded};
use cap_std::{ambient_authority, fs::Dir};
use std::{
    io::{self, Write},
    path::Path,
};

pub(crate) fn has_entry(path: &Path) -> io::Result<bool> {
    Ok(repository_directory(path)?.is_some())
}

fn repository_directory(path: &Path) -> io::Result<Option<&Path>> {
    for ancestor in path.ancestors() {
        match ancestor.join(".git").symlink_metadata() {
            Ok(_) => return Ok(Some(ancestor)),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

pub(super) async fn exclude_marker(path: &Path) -> io::Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let repository = discover(&path)?;
        // Preserve the final component for the no-follow writer.
        append_exclusion(&repository.common_dir().join("info/exclude"))
    })
    .await
    .map_err(io::Error::other)?
}

/// Repository-local metadata only: no Git executable, ambient discovery overrides,
/// global configuration or config includes. Handles remain on the blocking worker.
pub(crate) fn discover(path: &Path) -> io::Result<gix::Repository> {
    // Open the nearest entry explicitly: invalid nested metadata must not make
    // discovery silently select a different enclosing repository.
    let directory =
        repository_directory(path)?.ok_or_else(|| invalid("workspace has no Git repository"))?;
    gix::open::Options::isolated()
        .strict_config(true)
        .open(directory)
        .map(|repository| repository.to_thread_local())
        .map_err(|error| invalid(&format!("workspace Git discovery failed: {error}")))
}

fn append_exclusion(path: &Path) -> io::Result<()> {
    let dir = Dir::open_ambient_dir(
        path.parent()
            .ok_or_else(|| invalid("Git exclude has no parent"))?,
        ambient_authority(),
    )?;
    let name = path
        .file_name()
        .ok_or_else(|| invalid("Git exclude has no file name"))?;
    let mut file = dir.open_with(name, options().read(true).append(true).create(true))?;
    let bytes = read_bounded(&dir, name, &mut file, 1024 * 1024)?;
    if bytes
        .split(|b| *b == b'\n')
        .any(|line| line.strip_suffix(b"\r").unwrap_or(line) == MARKER_FILE.as_bytes())
    {
        return Ok(());
    }
    let addition = format!(
        "{}{MARKER_FILE}\n",
        if bytes.is_empty() || bytes.ends_with(b"\n") {
            ""
        } else {
            "\n"
        }
    );
    if bytes.len() + addition.len() > 1024 * 1024 {
        return Err(invalid("Git exclude exceeds capacity"));
    }
    file.write_all(addition.as_bytes())?;
    file.sync_all()
}
