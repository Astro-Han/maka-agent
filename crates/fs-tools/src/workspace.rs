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

//! Intrinsic workspace identity. Paths and inode numbers only detect observation races.

use cap_fs_ext::{FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::{
    ambient_authority,
    fs::{Dir, File, Metadata, OpenOptions},
};
use maka_runtime::execution::WorkspaceIdentity;
use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub mod directory;
mod git;
pub mod project;

pub const MARKER_FILE: &str = ".maka-workspace.json";
const MAX_MARKER_BYTES: usize = 4096;

/// Read-only safety observation: never creates, repairs or rebinds a marker.
pub fn read_identity(path: &Path) -> io::Result<WorkspaceIdentity> {
    Workspace::capture(path)?.read()
}

/// Prepare an execution workspace. A concurrent first publisher wins; an existing
/// malformed or changing marker is an error, not permission to mint another identity.
pub async fn ensure_identity(path: &Path) -> io::Result<WorkspaceIdentity> {
    let path = path.to_owned();
    let (workspace, existing, in_git) = tokio::task::spawn_blocking(move || {
        let workspace = Workspace::capture(&path)?;
        let existing = match workspace.read() {
            Ok(identity) => Some(identity),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let in_git = git::has_entry(&workspace.path)?;
        Ok::<_, io::Error>((workspace, existing, in_git))
    })
    .await
    .map_err(io::Error::other)??;
    if in_git {
        git::exclude_marker(&workspace.path).await?;
    }
    tokio::task::spawn_blocking(move || {
        workspace.validate_directory()?;
        if existing.is_none() {
            workspace.publish()?;
        }
        let observed = workspace.read()?;
        if existing.is_some_and(|expected| expected != observed) {
            return Err(invalid("workspace marker changed during preparation"));
        }
        Ok(observed)
    })
    .await
    .map_err(io::Error::other)?
}

struct Workspace {
    path: PathBuf,
    dir: Dir,
}

impl Workspace {
    fn capture(path: &Path) -> io::Result<Self> {
        let path = path.canonicalize()?;
        let dir = Dir::open_ambient_dir(&path, ambient_authority())?;
        let workspace = Self { path, dir };
        workspace.validate_directory()?;
        Ok(workspace)
    }

    fn validate_directory(&self) -> io::Result<()> {
        let visible = Dir::open_ambient_dir(&self.path, ambient_authority())?;
        if identity(&visible.dir_metadata()?) != identity(&self.dir.dir_metadata()?) {
            return Err(invalid("workspace directory changed"));
        }
        Ok(())
    }

    fn read(&self) -> io::Result<WorkspaceIdentity> {
        let file = self.dir.open_with(MARKER_FILE, options().read(true))?;
        self.read_opened(file)
    }

    fn read_opened(&self, mut file: File) -> io::Result<WorkspaceIdentity> {
        let bytes = read_bounded(
            &self.dir,
            OsStr::new(MARKER_FILE),
            &mut file,
            MAX_MARKER_BYTES,
        )
        .map_err(after_open)?;
        self.validate_directory().map_err(after_open)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("invalid workspace marker"))?;
        if object.len() != 2 || object.get("schemaVersion").and_then(|v| v.as_f64()) != Some(1.0) {
            return Err(invalid("invalid workspace marker schema"));
        }
        let id = object
            .get("workspaceId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid("invalid workspace marker UUID"))?;
        WorkspaceIdentity::from_marker_id(id).map_err(invalid)
    }

    fn publish(&self) -> io::Result<()> {
        let temp = format!("{MARKER_FILE}.{}.tmp", Uuid::new_v4());
        let mut file = self
            .dir
            .open_with(&temp, options().write(true).create_new(true))?;
        let result = (|| {
            let marker =
                serde_json::json!({"schemaVersion": 1, "workspaceId": Uuid::new_v4().to_string()});
            file.write_all(serde_json::to_string(&marker)?.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            self.validate_directory()?;
            match self.dir.hard_link(&temp, &self.dir, MARKER_FILE) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            }
        })();
        // Remove only our exact random candidate, even if publication failed.
        let cleanup = self.dir.remove_file(&temp);
        result?;
        cleanup?;
        #[cfg(unix)]
        // A capability Dir may hold O_PATH on Linux, which cannot be fsynced.
        // Open a readable descriptor relative to the captured directory itself.
        self.dir.open(".")?.sync_all()?;
        self.validate_directory()
    }
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.follow(FollowSymlinks::No);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NONBLOCK);
    options
}

fn identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

fn read_bounded(dir: &Dir, name: &OsStr, file: &mut File, max: usize) -> io::Result<Vec<u8>> {
    let before = file.metadata()?;
    let visible = dir.symlink_metadata(name)?;
    if !before.is_file()
        || !visible.is_file()
        || before.len() > max as u64
        || identity(&before) != identity(&visible)
    {
        return Err(invalid("marker must be one bounded regular file"));
    }
    let mut bytes = Vec::new();
    (&mut *file).take(max as u64 + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let visible = dir.symlink_metadata(name)?;
    if bytes.len() > max
        || bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || before.modified()? != after.modified()?
        || !visible.is_file()
        || identity(&before) != identity(&visible)
    {
        return Err(invalid("marker changed while reading"));
    }
    Ok(bytes)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn after_open(error: io::Error) -> io::Error {
    if matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    ) {
        invalid("opened workspace marker disappeared")
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disappearance_after_open_cannot_authorize_a_new_identity() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = Workspace::capture(temp.path()).unwrap();
        workspace.publish().unwrap();
        let file = workspace
            .dir
            .open_with(MARKER_FILE, options().read(true))
            .unwrap();
        workspace.dir.remove_file(MARKER_FILE).unwrap();
        assert_eq!(
            workspace.read_opened(file).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(!temp.path().join(MARKER_FILE).exists());
    }
}
