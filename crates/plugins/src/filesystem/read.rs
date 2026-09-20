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

//! Read-only input capabilities; never execution authority.
use super::directory::Directory;
use crate::{
    Error,
    fiber::Context,
    filesystem::entries::{DirectoryPage, Entry, FilePage, Kind, ListFiles, ReadFile},
};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

/// Whether to resolve the final component's symlink inside the admitted root.
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Symlinks {
    #[default]
    Follow,
    Reject,
}
impl Symlinks {
    fn follow(self) -> bool {
        matches!(self, Self::Follow)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadInput {
    #[serde(flatten)]
    pub file: ReadFile,
    #[serde(default)]
    pub symlinks: Symlinks,
}
impl From<ReadFile> for ReadInput {
    fn from(file: ReadFile) -> Self {
        Self {
            file,
            symlinks: Symlinks::Follow,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListInput {
    #[serde(flatten)]
    pub files: ListFiles,
    #[serde(default)]
    pub symlinks: Symlinks,
}
impl From<ListFiles> for ListInput {
    fn from(files: ListFiles) -> Self {
        Self {
            files,
            symlinks: Symlinks::Follow,
        }
    }
}
/// I/O classifications survive the native API; missing optional inputs are not failures.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("read capability is retired")]
    Retired,
    #[error("invalid read operation: {0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}
impl From<ReadError> for Error {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Retired => Self::Retired,
            error => Self::Invalid(error.to_string()),
        }
    }
}

/// Revalidate an authenticated read scope before each batch. Lifecycle alone
/// is not authorization; this contract is implemented by the embedding.
pub trait ReadAuthorization: Send + Sync {
    fn check(&self) -> futures_util::future::BoxFuture<'_, Result<crate::call::Ticket, ReadError>>;
}

/// Embedding-owned root. Only the embedding decides which directory to expose.
#[derive(Clone)]
pub struct ReadRoot {
    directory: Directory,
    files: Option<Arc<BTreeSet<String>>>,
}
impl ReadRoot {
    pub fn bind_authorized(
        &self,
        owner: Context,
        cancellation: CancellationToken,
        authorization: Arc<dyn ReadAuthorization>,
    ) -> ReadDirectory {
        let mut view = self.bind(owner, cancellation);
        view.authorization = Some(authorization);
        view
    }
    pub(crate) fn from_handle(directory: cap_std::fs::Dir, path: std::path::PathBuf) -> Self {
        Self {
            directory: Directory::from_handle(directory, path),
            files: None,
        }
    }
    pub async fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        tokio::task::spawn_blocking(move || {
            Directory::capture(&path).map(|directory| Self {
                directory,
                files: None,
            })
        })
        .await
        .map_err(io::Error::other)?
    }
    /// Restrict a mount to relative files or subtrees (names ending in '/').
    pub fn select(mut self, files: BTreeSet<String>) -> Result<Self, Error> {
        for path in &files {
            relative(path.trim_end_matches('/'))?;
        }
        self.files = Some(Arc::new(files));
        Ok(self)
    }
    /// Bind an embedding-authorized root to one callback or plugin lifetime.
    pub fn bind(&self, owner: Context, cancellation: CancellationToken) -> ReadDirectory {
        ReadDirectory {
            root: self.directory.clone(),
            files: self.files.clone(),
            owner,
            cancellation,
            authorization: None,
        }
    }
}

/// Explicitly shared, non-secret input mounts configured by the embedding.
/// This is not the Host state directory or an ambient home-directory grant.
#[derive(Clone, Default)]
pub struct ReadRoots(pub BTreeMap<String, ReadRoot>);
impl ReadRoots {
    pub fn bind(&self, owner: Context) -> ReadInputs {
        ReadInputs {
            roots: self.clone(),
            owner,
        }
    }
}
#[derive(Clone)]
pub struct ReadInputs {
    roots: ReadRoots,
    owner: Context,
}
impl ReadInputs {
    pub fn names(&self) -> Result<Vec<String>, Error> {
        let _lease = self.owner.resource_call()?;
        Ok(self.roots.0.keys().cloned().collect())
    }
    pub fn open(&self, name: &str) -> Result<Option<ReadDirectory>, Error> {
        let _lease = self.owner.resource_call()?;
        let cancellation = self.owner.stopping()?;
        Ok(self
            .roots
            .0
            .get(name)
            .map(|root| root.bind(self.owner.clone(), cancellation)))
    }
}

/// Relative paths, bounded reads and directory pages; no raw handles or writes.
/// Borrowed callback views expire when the callback ends.
#[derive(Clone)]
pub struct ReadDirectory {
    root: Directory,
    files: Option<Arc<BTreeSet<String>>>,
    owner: Context,
    cancellation: CancellationToken,
    authorization: Option<Arc<dyn ReadAuthorization>>,
}
impl ReadDirectory {
    /// Display location only. It never grants authority to reopen the pathname.
    pub fn location(&self) -> std::path::PathBuf {
        self.root.path().to_owned()
    }
    /// Batch native reads on one blocking worker. The borrowed adapter cannot
    /// escape, exposes the same bounded operations as JS, and checks cancellation
    /// on each operation. The resource lease remains with the worker if its caller drops.
    pub async fn with_reader<T: Send + 'static>(
        &self,
        work: impl FnOnce(Reader<'_>) -> T + Send + 'static,
    ) -> Result<T, ReadError> {
        if self.cancellation.is_cancelled() {
            return Err(ReadError::Retired);
        }
        let lease = self.owner.resource_call().map_err(|_| ReadError::Retired)?;
        let mut ticket = if let Some(authorization) = &self.authorization {
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Err(ReadError::Retired),
                result = authorization.check() => Some(result?),
            }
        } else {
            None
        };
        let view = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            let reader = Reader { view: &view };
            reader.check()?;
            if let Some(ticket) = &mut ticket {
                ticket.start();
            }
            let result = work(reader);
            if let Some(ticket) = ticket {
                ticket.complete(Ok(()));
            }
            Ok(result)
        })
        .await
        .map_err(|error| ReadError::Io(io::Error::other(error)))?
    }

    pub async fn read(&self, input: impl Into<ReadInput>) -> Result<FilePage, ReadError> {
        let input = input.into();
        self.with_reader(move |reader| reader.read(input)).await?
    }
    pub async fn list(&self, input: impl Into<ListInput>) -> Result<DirectoryPage, ReadError> {
        let input = input.into();
        self.with_reader(move |reader| reader.list(input)).await?
    }
}

/// A synchronous, borrowed read capability for native batch algorithms.
/// No mutable file, directory handle or ambient pathname is exposed.
pub struct Reader<'a> {
    view: &'a ReadDirectory,
}
impl Reader<'_> {
    fn check(&self) -> Result<(), ReadError> {
        if self.view.cancellation.is_cancelled() {
            Err(ReadError::Retired)
        } else {
            Ok(())
        }
    }
    pub fn read(&self, input: impl Into<ReadInput>) -> Result<FilePage, ReadError> {
        self.check()?;
        let ReadInput {
            file: input,
            symlinks,
        } = input.into();
        relative(&input.path)?;
        if self
            .view
            .files
            .as_ref()
            .is_some_and(|files| !selected(files, Path::new(&input.path)))
        {
            return Err(invalid("file is outside the mounted input selection"));
        }
        if input.limit == 0 || input.limit > 1024 * 1024 || input.offset > (1_u64 << 53) - 1 {
            return Err(invalid("invalid read range"));
        }
        let (parent, name) = self
            .view
            .root
            .resolve(Path::new(&input.path), symlinks.follow())?;
        if self.view.files.as_ref().is_some_and(|files| {
            let path = parent.relative(Path::new(&name));
            !selected(files, &path)
        }) {
            return Err(invalid(
                "link target is outside the mounted input selection",
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut file = parent.dir().open_with(name, &options)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("expected a regular file"));
        }
        file.seek(SeekFrom::Start(input.offset))?;
        let mut bytes = Vec::new();
        let mut reader = file.take(input.limit as u64 + 1);
        let mut chunk = [0; 8192];
        loop {
            self.check()?;
            let count = reader.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        let next_offset = (bytes.len() > input.limit).then_some(input.offset + input.limit as u64);
        bytes.truncate(input.limit);
        Ok(FilePage { bytes, next_offset })
    }

    pub fn list(&self, input: impl Into<ListInput>) -> Result<DirectoryPage, ReadError> {
        self.check()?;
        let ListInput {
            files: input,
            symlinks,
        } = input.into();
        if !input.path.is_empty() {
            relative(&input.path)?;
        }
        if input.limit == 0
            || input.limit > 1024
            || input.after.as_ref().is_some_and(|s| s.len() > 4096)
        {
            return Err(invalid("invalid directory page"));
        }
        let directory = self
            .view
            .root
            .open(Path::new(&input.path), symlinks.follow())?;
        if self
            .view
            .files
            .as_ref()
            .is_some_and(|files| !visible(files, &directory.relative(Path::new(""))))
        {
            return Err(invalid("directory is outside the mounted input selection"));
        }
        let mut entries = BTreeMap::new();
        for entry in directory.dir().entries()? {
            self.check()?;
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("filename is not UTF-8"))?;
            let path = if input.path.is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", input.path)
            };
            if self.view.files.as_ref().is_some_and(|files| {
                !visible(files, Path::new(&path))
                    || !visible(files, &directory.relative(Path::new(&name)))
            }) {
                continue;
            }
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
    }
}
fn relative(path: &str) -> Result<(), ReadError> {
    if path.is_empty()
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || path.contains(['\\', ':'])
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid("expected a relative read-view path"));
    }
    Ok(())
}
fn selected(files: &BTreeSet<String>, path: &Path) -> bool {
    files.iter().any(|file| {
        if file.ends_with('/') {
            path.starts_with(file.trim_end_matches('/'))
        } else {
            Path::new(file) == path
        }
    })
}
fn visible(files: &BTreeSet<String>, path: &Path) -> bool {
    selected(files, path)
        || files
            .iter()
            .any(|file| Path::new(file.trim_end_matches('/')).starts_with(path))
}
fn invalid(error: impl ToString) -> ReadError {
    ReadError::Invalid(error.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{composition::Scope, fiber::Fiber};

    #[tokio::test]
    async fn reads_keep_mount_boundaries_and_end_with_the_callback_or_owner() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("allowed"), b"abcdef").unwrap();
        std::fs::write(directory.path().join("private"), b"secret").unwrap();
        let owner = Fiber::new("example.reader", "reader", Scope::Profile).unwrap();
        owner.begin_loading().unwrap();
        owner.ready().unwrap();
        owner.publish().unwrap();
        let root = ReadRoot::open(directory.path()).await.unwrap();
        let restricted = root.clone().select(["allowed".into()].into()).unwrap();
        let cancellation = CancellationToken::new();
        let view = restricted.bind(owner.context(), cancellation.clone());
        let read = |path: &str| ReadFile {
            path: path.into(),
            offset: 0,
            limit: 3,
        };
        let page = view.read(read("allowed")).await.unwrap();
        assert_eq!(page.bytes, b"abc");
        assert_eq!(page.next_offset, Some(3));
        assert!(view.read(read("private")).await.is_err());
        let page = view
            .list(ListFiles {
                path: String::new(),
                after: None,
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["allowed"]
        );
        let workspace = root.bind(owner.context(), cancellation.clone());
        assert!(workspace.read(read("../private")).await.is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret"), b"outside").unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("secret"),
                directory.path().join("escape"),
            )
            .unwrap();
            assert!(workspace.read(read("escape")).await.is_err());
            std::os::unix::fs::symlink("allowed", directory.path().join("alias")).unwrap();
            assert_eq!(workspace.read(read("alias")).await.unwrap().bytes, b"abc");
            let reject: ReadInput = serde_json::from_value(serde_json::json!({
                "path": "alias", "limit": 3, "symlinks": "reject"
            }))
            .unwrap();
            assert!(workspace.read(reject).await.is_err());
            std::os::unix::fs::symlink(
                directory.path().join("allowed"),
                directory.path().join("absolute"),
            )
            .unwrap();
            assert_eq!(
                workspace.read(read("absolute")).await.unwrap().bytes,
                b"abc"
            );
            let selected_alias = root
                .clone()
                .select(["alias".into()].into())
                .unwrap()
                .bind(owner.context(), cancellation.clone());
            assert!(
                selected_alias.read(read("alias")).await.is_err(),
                "selection must constrain link targets too"
            );

            let moved = directory.path().with_extension("retained");
            std::fs::rename(directory.path(), &moved).unwrap();
            std::fs::create_dir(directory.path()).unwrap();
            std::fs::write(directory.path().join("allowed"), "wrong").unwrap();
            assert_eq!(
                workspace.read(read("absolute")).await.unwrap().bytes,
                b"abc"
            );
            // Return the fixture to its temporary owner for automatic cleanup.
            std::fs::remove_file(directory.path().join("allowed")).unwrap();
            std::fs::remove_dir(directory.path()).unwrap();
            std::fs::rename(moved, directory.path()).unwrap();
        }
        cancellation.cancel();
        assert!(matches!(
            view.read(read("allowed")).await,
            Err(ReadError::Retired)
        ));
        let published =
            ReadRoots(BTreeMap::from([("notes".into(), restricted)])).bind(owner.context());
        let independent = published.open("notes").unwrap().unwrap();
        assert_eq!(
            independent.read(read("allowed")).await.unwrap().bytes,
            b"abc"
        );
        owner
            .shutdown(tokio::time::Instant::now() + std::time::Duration::from_secs(1))
            .await
            .unwrap();
        assert!(matches!(
            independent.read(read("allowed")).await,
            Err(ReadError::Retired)
        ));
        assert!(matches!(published.names(), Err(Error::Retired)));
    }
}
