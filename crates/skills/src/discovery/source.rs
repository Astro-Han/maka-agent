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
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use std::{
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::{Component, Path},
};
use tokio_util::sync::CancellationToken;

const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

pub(super) struct Captured {
    directory: super::directory::Directory,
}

pub(super) enum EntriesError {
    Scan(ScanError),
    Read,
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

impl Captured {
    pub(super) fn open(source: &Source) -> Result<Option<Self>, DiscoveryFailure> {
        if !source.root.is_absolute()
            || source
                .directory
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(DiscoveryFailure::BlockedPath);
        }
        let result = Self::capture(source);
        match result {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(classify(&error)),
            Ok(captured) => Ok(Some(captured)),
        }
    }

    fn capture(source: &Source) -> io::Result<Self> {
        if (source.directory.as_os_str().is_empty() || source.directory == Path::new("."))
            && source.root.symlink_metadata()?.file_type().is_symlink()
        {
            return Err(blocked());
        }
        Ok(Self {
            directory: super::directory::Directory::capture(&source.root, &source.directory)?,
        })
    }

    pub(super) fn entries(
        &self,
        remaining: &mut usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<OsString>, EntriesError> {
        let mut names = Vec::new();
        for entry in self
            .directory
            .dir()
            .entries()
            .map_err(|_| EntriesError::Read)?
        {
            check_cancelled(cancellation).map_err(EntriesError::Scan)?;
            *remaining = remaining
                .checked_sub(1)
                .ok_or(EntriesError::Scan(ScanError::LimitExceeded))?;
            names.push(entry.map_err(|_| EntriesError::Read)?.file_name());
        }
        names.sort_unstable();
        Ok(names)
    }

    pub(super) fn read_skill(
        &self,
        name: &OsStr,
        collect_origin: bool,
        cancellation: &CancellationToken,
    ) -> Result<Option<SkillRead>, ReadError> {
        check_cancelled(cancellation).map_err(|_| ReadError::Cancelled)?;
        let metadata = self
            .directory
            .dir()
            .symlink_metadata(name)
            .map_err(read_error)?;
        if !metadata.is_dir() && !metadata.file_type().is_symlink() {
            return Ok(None);
        }
        let directory = match self.directory.child(Path::new(name)) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotADirectory => return Ok(None),
            Err(error) => return Err(read_error(error)),
        };
        let Some(bytes) = read_file(&directory, "SKILL.md", MAX_SOURCE_BYTES, cancellation)? else {
            return Ok(metadata.is_dir().then_some(SkillRead::Empty));
        };
        let (origin, origin_bytes) = if collect_origin {
            let (origin, size) = super::origin::read(
                &directory,
                name.to_str().expect("caller admitted UTF-8 skill identity"),
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

pub(super) fn read_file(
    directory: &super::directory::Directory,
    name: &str,
    limit: u64,
    cancellation: &CancellationToken,
) -> Result<Option<Vec<u8>>, ReadError> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match directory.dir().open_with(name, &options) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(read_error(error)),
    };
    let metadata = file.metadata().map_err(read_error)?;
    if !metadata.is_file() {
        return Err(ReadError::Failure(DiscoveryFailure::BlockedPath));
    }
    if metadata.len() > limit {
        return Err(ReadError::Failure(DiscoveryFailure::SourceTooLarge));
    }
    let mut bytes = Vec::new();
    let mut reader = file.take(limit + 1);
    let mut chunk = [0u8; 8192];
    loop {
        check_cancelled(cancellation).map_err(|_| ReadError::Cancelled)?;
        let count = reader.read(&mut chunk).map_err(read_error)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() as u64 > limit {
            return Err(ReadError::Failure(DiscoveryFailure::SourceTooLarge));
        }
    }
    Ok(Some(bytes))
}

fn blocked() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "skill path is outside its admitted directory",
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_directory_survives_ambient_root_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let root_path = temporary.path().join("root");
        std::fs::create_dir_all(root_path.join("skills/review")).unwrap();
        std::fs::write(root_path.join("skills/review/SKILL.md"), "original").unwrap();
        let hash = maka_runtime::artifact::content_digest(b"original");
        std::fs::write(
            root_path.join("skills/review/skill.lock.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1, "id": "review", "sourceType": "managed",
                "sourceName": "local-library", "sourceVersion": "1", "sourceId": "source",
                "contentSha256": hash, "sourceContentSha256": hash,
            }))
            .unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            for (name, content) in [("a", "original link"), ("b", "wrong target")] {
                std::fs::create_dir_all(root_path.join("store").join(name)).unwrap();
                std::fs::write(root_path.join("store").join(name).join("SKILL.md"), content)
                    .unwrap();
            }
            std::os::unix::fs::symlink(root_path.join("store/a"), root_path.join("skills/linked"))
                .unwrap();
        }
        let source = Source::at(
            &root_path,
            "skills",
            maka_runtime::skills::SkillScope::Custom,
            maka_runtime::skills::SkillSource::Custom,
            "custom:test",
        );
        let captured = Captured::open(&source).unwrap().unwrap();
        let retained = temporary.path().join("retained");
        let replacement = std::fs::rename(&root_path, &retained);
        #[cfg(windows)]
        assert_eq!(
            replacement.unwrap_err().raw_os_error(),
            Some(32),
            "captured Windows directories deny replacement while handles remain open"
        );
        #[cfg(unix)]
        {
            replacement.unwrap();
            std::fs::create_dir_all(root_path.join("skills/review")).unwrap();
            std::fs::write(root_path.join("skills/review/SKILL.md"), "replacement").unwrap();
            std::fs::create_dir_all(root_path.join("store/b")).unwrap();
            std::os::unix::fs::symlink(root_path.join("store/b"), root_path.join("skills/linked"))
                .unwrap();
            let content = captured
                .read_skill(OsStr::new("linked"), false, &CancellationToken::new())
                .ok()
                .flatten()
                .unwrap();
            let SkillRead::Document { bytes, .. } = content else {
                panic!("missing captured body")
            };
            assert_eq!(bytes, b"original link");
        }
        let content = captured
            .read_skill(OsStr::new("review"), true, &CancellationToken::new())
            .ok()
            .flatten()
            .unwrap();
        let SkillRead::Document { bytes, origin, .. } = content else {
            panic!("missing captured body")
        };
        assert_eq!(bytes, b"original");
        assert!(matches!(origin.unwrap().status,
            super::super::origin::OriginStatus::Managed { source_id, content_sha256 }
                if source_id == "source" && content_sha256 == hash));
        #[cfg(windows)]
        {
            drop(captured);
            std::fs::rename(&root_path, &retained).unwrap();
        }
    }
}
