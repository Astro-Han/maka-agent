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

//! Bounded private-file operations shared by language bindings. File formats,
//! multi-operation transactions and coordination belong to the plugin.

use super::{Directory, StoreError};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Read, Seek, SeekFrom, Write},
};

const CHUNK: usize = 1024 * 1024;
const MAX_OFFSET: u64 = (1_u64 << 53) - 1;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadFile {
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "read_limit")]
    pub limit: usize,
}
fn read_limit() -> usize {
    64 * 1024
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePage {
    pub bytes: Vec<u8>,
    pub next_offset: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteFile {
    pub path: String,
    #[serde(default)]
    pub offset: u64,
    pub bytes: Vec<u8>,
    /// Explicitly truncate before writing; only valid at offset zero.
    #[serde(default)]
    pub truncate: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListFiles {
    /// Empty selects the namespace root; mutations never accept the root.
    #[serde(default)]
    pub path: String,
    pub after: Option<String>,
    #[serde(default = "list_limit")]
    pub limit: usize,
}
fn list_limit() -> usize {
    256
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryPage {
    pub entries: Vec<Entry>,
    pub next_after: Option<String>,
}
#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Directory,
    Other,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid private-file operation: {0}")]
    Invalid(String),
    #[error("private file does not exist")]
    NotFound,
    #[error("private file already exists")]
    AlreadyExists,
    #[error("private-file capability is retired")]
    Retired,
    #[error("private-file I/O failed: {0}")]
    Io(String),
    #[error("private-file mutation outcome is unknown: {0}")]
    OutcomeUnknown(String),
}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            _ => Self::Io(error.to_string()),
        }
    }
}
impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Retired => Self::Retired,
            StoreError::OutcomeUnknown(message) => Self::OutcomeUnknown(message),
            error => Self::Io(error.to_string()),
        }
    }
}

impl Directory {
    pub async fn read(&self, input: ReadFile) -> Result<FilePage, Error> {
        path(&input.path)?;
        if input.limit == 0 || input.limit > CHUNK || input.offset > MAX_OFFSET {
            return Err(Error::Invalid("invalid read range".into()));
        }
        self.run(move |root| {
            let (parent, name) = parent(root, &input.path)?;
            let mut file = parent.open_with(name, options().read(true))?;
            if !file.metadata()?.is_file() {
                return Err(Error::Invalid("expected a regular file".into()));
            }
            file.seek(SeekFrom::Start(input.offset))?;
            let mut bytes = Vec::new();
            file.take(input.limit as u64 + 1).read_to_end(&mut bytes)?;
            let next_offset =
                (bytes.len() > input.limit).then_some(input.offset + input.limit as u64);
            bytes.truncate(input.limit);
            Ok(FilePage { bytes, next_offset })
        })
        .await?
    }

    /// Flushes a bounded write, not an atomic replacement. On uncertain failure
    /// the plugin must inspect/recover its file instead of assuming no mutation.
    pub async fn write(&self, input: WriteFile) -> Result<(), Error> {
        path(&input.path)?;
        if input.bytes.len() > CHUNK
            || input.offset > MAX_OFFSET - input.bytes.len() as u64
            || (input.truncate && input.offset != 0)
        {
            return Err(Error::Invalid("invalid write range".into()));
        }
        self.run(move |root| {
            let (parent, name) = parent(root, &input.path)?;
            let mut file = parent.open_with(name, options().write(true).create(true))?;
            if !file.metadata()?.is_file() {
                return Err(Error::Invalid("expected a regular file".into()));
            }
            let mut write = || -> io::Result<()> {
                if input.truncate {
                    file.set_len(0)?;
                }
                file.seek(SeekFrom::Start(input.offset))?;
                file.write_all(&input.bytes)?;
                file.sync_all()?;
                sync(&parent)
            };
            write().map_err(|error| Error::OutcomeUnknown(error.to_string()))
        })
        .await?
    }

    pub async fn list(&self, input: ListFiles) -> Result<DirectoryPage, Error> {
        if !input.path.is_empty() {
            path(&input.path)?;
        }
        if input.limit == 0
            || input.limit > 1024
            || input.after.as_ref().is_some_and(|s| s.len() > 4096)
        {
            return Err(Error::Invalid("invalid directory page limit".into()));
        }
        self.run(move |root| {
            let directory = directory(root, &input.path)?;
            let mut entries = BTreeMap::new();
            for entry in directory.entries()? {
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::Invalid("filename is not UTF-8".into()))?;
                if input.after.as_ref().is_some_and(|after| &name <= after) {
                    continue;
                }
                let kind = entry.file_type()?;
                let kind = if kind.is_file() {
                    Kind::File
                } else if kind.is_dir() {
                    Kind::Directory
                } else {
                    Kind::Other
                };
                entries.insert(name.clone(), Entry { name, kind });
                if entries.len() > input.limit + 1 {
                    entries.pop_last();
                }
            }
            let next_after = if entries.len() > input.limit {
                entries.pop_last();
                entries.last_key_value().map(|(name, _)| name.clone())
            } else {
                None
            };
            Ok(DirectoryPage {
                entries: entries.into_values().collect(),
                next_after,
            })
        })
        .await?
    }

    pub async fn create_directory(&self, name: String) -> Result<(), Error> {
        path(&name)?;
        self.run(move |root| {
            let (parent, name) = parent(root, &name)?;
            parent.create_dir(name)?;
            sync(&parent).map_err(|error| Error::OutcomeUnknown(error.to_string()))
        })
        .await?
    }

    /// Removes a file/link or an empty directory, never a recursive tree.
    pub async fn remove(&self, name: String) -> Result<(), Error> {
        path(&name)?;
        self.run(move |root| {
            let (parent, name) = parent(root, &name)?;
            if parent.symlink_metadata(name)?.is_dir() {
                parent.remove_dir(name)?;
            } else {
                parent.remove_file(name)?;
            }
            sync(&parent).map_err(|error| Error::OutcomeUnknown(error.to_string()))
        })
        .await?
    }

    /// Rename within the namespace, replacing an existing file. The filesystem
    /// operation is atomic, not a portable power-loss durability guarantee.
    pub async fn rename(&self, from: String, to: String) -> Result<(), Error> {
        path(&from)?;
        path(&to)?;
        self.run(move |root| {
            let (source, name) = parent(root, &from)?;
            let (target, destination) = parent(root, &to)?;
            source.rename(name, &target, destination)?;
            sync(&target)
                .and_then(|()| sync(&source))
                .map_err(|error| Error::OutcomeUnknown(error.to_string()))
        })
        .await?
    }
}

fn path(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 4096
        || value.chars().any(char::is_control)
        || value.contains(['\\', ':', '*', '?', '"', '<', '>', '|'])
        || value
            .split('/')
            .any(|part| part.is_empty() || part.ends_with(['.', ' ']) || reserved(part))
    {
        return Err(Error::Invalid("expected a relative namespace path".into()));
    }
    Ok(())
}
fn reserved(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or_default();
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            upper.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
}
fn directory(root: &Dir, path: &str) -> Result<Dir, Error> {
    let mut directory = root.try_clone()?;
    if !path.is_empty() {
        for part in path.split('/') {
            directory = directory.open_dir_nofollow(part)?;
        }
    }
    Ok(directory)
}
fn parent<'a>(root: &Dir, path: &'a str) -> Result<(Dir, &'a str), Error> {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    Ok((directory(root, parent)?, name))
}
fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options
}
fn sync(directory: &Dir) -> io::Result<()> {
    #[cfg(unix)]
    {
        directory.try_clone()?.into_std_file().sync_all()
    }
    #[cfg(windows)]
    {
        // File contents are flushed. There is no portable directory fsync;
        // namespace mutations do not promise persistence across power loss.
        let _ = directory;
        Ok(())
    }
}
