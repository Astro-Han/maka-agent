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

use super::{DiscoveryFailure, ScanError, Source, check_cancelled};
use maka_plugins::{
    filesystem::entries::{Kind, ReadFile},
    filesystem::{ReadError as FileError, ReadViewInput, Reader, Symlinks},
};
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    io,
    path::{Component, Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub(super) struct Captured<'a, 'view> {
    reader: &'a Reader<'view>,
    path: PathBuf,
    directories: BTreeSet<OsString>,
    files: BTreeSet<OsString>,
}
pub(super) enum EntriesError {
    Scan(ScanError),
    Read(DiscoveryFailure),
}
impl From<FileError> for EntriesError {
    fn from(error: FileError) -> Self {
        match error {
            FileError::Retired => Self::Scan(ScanError::Cancelled),
            FileError::ScanLimit { .. } => Self::Scan(ScanError::LimitExceeded),
            FileError::Io(error) => Self::Read(classify(&error)),
            FileError::Invalid(_) => Self::Read(DiscoveryFailure::BlockedPath),
        }
    }
}
pub(super) enum ReadError {
    Cancelled,
    Failure(DiscoveryFailure),
}
pub(super) enum SkillRead {
    Empty,
    Document {
        bytes: Vec<u8>,
        origin: Option<super::origin::Origin>,
        origin_bytes: usize,
    },
}
impl<'a, 'view> Captured<'a, 'view> {
    pub(super) fn open(
        source: &Source,
        reader: &'a Reader<'view>,
    ) -> Result<Self, DiscoveryFailure> {
        if source
            .directory
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(DiscoveryFailure::BlockedPath);
        }
        Ok(Self {
            reader,
            path: source.directory.clone(),
            directories: BTreeSet::new(),
            files: BTreeSet::new(),
        })
    }

    pub(super) fn entries(
        &mut self,
        remaining: &mut usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<OsString>, EntriesError> {
        let mut names = Vec::new();
        let path =
            portable(&self.path).map_err(|_| EntriesError::Read(DiscoveryFailure::BlockedPath))?;
        let budget = *remaining as u64;
        let result = self
            .reader
            .visit_directory(&path, Symlinks::Reject, |entry| {
                check_cancelled(cancellation).map_err(|_| FileError::Retired)?;
                *remaining = remaining
                    .checked_sub(1)
                    .ok_or(FileError::ScanLimit { max: budget })?;
                let name = OsString::from(entry.name);
                if matches!(entry.kind, Kind::Directory) {
                    self.directories.insert(name.clone());
                }
                if matches!(entry.kind, Kind::File) {
                    self.files.insert(name.clone());
                }
                names.push(name);
                Ok::<_, FileError>(())
            });
        match result {
            Ok(()) => {}
            Err(FileError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        names.sort();
        Ok(names)
    }

    pub(super) fn read_skill(
        &self,
        name: &OsStr,
        collect_origin: bool,
        cancellation: &CancellationToken,
    ) -> Result<Option<SkillRead>, ReadError> {
        if self.files.contains(name) {
            return Ok(None);
        }
        let directory = self.path.join(name);
        let bytes = read_view(
            self.reader,
            &directory.join("SKILL.md"),
            1024 * 1024,
            cancellation,
        )?;
        let Some(bytes) = bytes else {
            return Ok(self.directories.contains(name).then_some(SkillRead::Empty));
        };
        let (origin, origin_bytes) = if collect_origin {
            let (origin, size) = super::origin::read(
                self.reader,
                &directory,
                name.to_str().expect("UTF-8 entry"),
                cancellation,
            )?;
            (Some(origin), size)
        } else {
            (None, 0)
        };
        Ok(Some(SkillRead::Document {
            bytes,
            origin,
            origin_bytes,
        }))
    }
}

pub(super) fn read_view(
    reader: &Reader<'_>,
    path: &Path,
    limit: usize,
    cancellation: &CancellationToken,
) -> Result<Option<Vec<u8>>, ReadError> {
    check_cancelled(cancellation).map_err(|_| ReadError::Cancelled)?;
    let input = ReadViewInput {
        file: ReadFile {
            path: portable(path).map_err(|_| ReadError::Failure(DiscoveryFailure::BlockedPath))?,
            offset: 0,
            limit,
        },
        symlinks: Symlinks::Reject,
    };
    match reader.read(input) {
        Ok(page) if page.next_offset.is_some() => {
            Err(ReadError::Failure(DiscoveryFailure::SourceTooLarge))
        }
        Ok(page) => Ok(Some(page.bytes)),
        Err(FileError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(FileError::Io(error)) => Err(read_error(error)),
        Err(FileError::Retired) => Err(ReadError::Cancelled),
        Err(FileError::Invalid(_)) => Err(ReadError::Failure(DiscoveryFailure::BlockedPath)),
        Err(FileError::ScanLimit { .. }) => {
            Err(ReadError::Failure(DiscoveryFailure::SourceTooLarge))
        }
    }
}
fn portable(path: &Path) -> Result<String, ()> {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| match c {
            Component::Normal(name) => name.to_str().ok_or(()),
            _ => Err(()),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

fn classify(error: &io::Error) -> DiscoveryFailure {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return DiscoveryFailure::BlockedPath;
    }
    if matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::NotADirectory
    ) {
        DiscoveryFailure::BlockedPath
    } else {
        DiscoveryFailure::ReadFailed
    }
}
fn read_error(error: io::Error) -> ReadError {
    ReadError::Failure(classify(&error))
}
