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

use super::Error;
use maka_plugins::{
    call::Scope,
    filesystem::{
        self,
        entries::{self, Kind, Operation, Output},
    },
};
use std::sync::Arc;

/// Domain adapter: the publication algorithm is identical for private files and
/// user-authorized files. Only private data carries a native directory handle.
#[derive(Clone)]
pub(super) struct Directory {
    root: Arc<Root>,
    prefix: String,
}
enum Root {
    Private {
        directory: cap_std::fs::Dir,
        cancellation: tokio_util::sync::CancellationToken,
    },
    Granted {
        files: Arc<dyn filesystem::Files>,
        call: Scope,
    },
}
impl Directory {
    pub fn private(
        directory: cap_std::fs::Dir,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            root: Arc::new(Root::Private {
                directory,
                cancellation,
            }),
            prefix: String::new(),
        }
    }
    pub fn granted(files: Arc<dyn filesystem::Files>, call: Scope) -> Self {
        Self {
            root: Arc::new(Root::Granted { files, call }),
            prefix: String::new(),
        }
    }
    fn path(&self, name: &str) -> String {
        match (self.prefix.is_empty(), name.is_empty()) {
            (true, _) => name.into(),
            (_, true) => self.prefix.clone(),
            _ => format!("{}/{name}", self.prefix),
        }
    }
    pub fn at(&self, name: &str) -> Self {
        Self {
            root: self.root.clone(),
            prefix: self.path(name),
        }
    }
    async fn invoke(&self, operation: Operation) -> Result<Output, Error> {
        match self.root.as_ref() {
            Root::Private { .. } => {
                let root = self.root.clone();
                tokio::task::spawn_blocking(move || {
                    let Root::Private {
                        directory,
                        cancellation,
                    } = root.as_ref()
                    else {
                        unreachable!()
                    };
                    entries::execute(directory, operation, cancellation, None)
                })
                .await
                .map_err(|e| Error::OutcomeUnknown(e.to_string()))?
                .map_err(Into::into)
            }
            Root::Granted { files, call } => {
                match files
                    .invoke(call.clone(), filesystem::Operation::Entries(operation))
                    .await?
                {
                    filesystem::Output::Entries(output) => Ok(output),
                    _ => Err(Error::Invalid("Invalid filesystem output".into())),
                }
            }
        }
    }
    pub async fn open(&self, name: &str) -> Result<Self, Error> {
        let path = self.path(name);
        let output = self.invoke(Operation::Stat { path: path.clone() }).await?;
        if !matches!(
            output,
            Output::Stat(entries::Metadata {
                kind: Kind::Directory,
                ..
            })
        ) {
            return Err(Error::Invalid("Expected a Skill directory".into()));
        }
        Ok(Self {
            root: self.root.clone(),
            prefix: path,
        })
    }
    pub async fn child(&self, name: &str) -> Result<Self, Error> {
        match self.create_dir(name).await {
            Ok(()) => {}
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        self.open(name).await
    }
    pub async fn create_dir(&self, name: &str) -> Result<(), Error> {
        self.invoke(Operation::CreateDirectory {
            path: self.path(name),
        })
        .await?;
        Ok(())
    }
    pub async fn entries(&self) -> Result<Vec<entries::Entry>, Error> {
        let Output::List(page) = self
            .invoke(Operation::List(entries::ListFiles {
                path: self.prefix.clone(),
                after: None,
                limit: 1024,
            }))
            .await?
        else {
            return Err(Error::Invalid("Invalid directory output".into()));
        };
        if page.next_after.is_some() {
            return Err(Error::Invalid("Skill directory exceeds entry limit".into()));
        }
        Ok(page.entries)
    }
    pub async fn read(&self, name: &str, limit: usize) -> Result<(Vec<u8>, u32), Error> {
        let path = self.path(name);
        let Output::Stat(metadata) = self.invoke(Operation::Stat { path: path.clone() }).await?
        else {
            return Err(Error::Invalid("Invalid metadata output".into()));
        };
        if !matches!(metadata.kind, Kind::File) || metadata.size > limit as u64 {
            return Err(Error::Invalid(
                "Skill resource is not a bounded regular file".into(),
            ));
        }
        let mut bytes = Vec::new();
        loop {
            let Output::Read(page) = self
                .invoke(Operation::Read(entries::ReadFile {
                    path: path.clone(),
                    offset: bytes.len() as u64,
                    limit: (limit + 1 - bytes.len()).min(1024 * 1024),
                }))
                .await?
            else {
                return Err(Error::Invalid("Invalid file output".into()));
            };
            bytes.extend(page.bytes);
            if bytes.len() > limit {
                return Err(Error::Invalid(
                    "Skill resource grew beyond its limit".into(),
                ));
            }
            if page.next_offset.is_none() {
                return Ok((bytes, metadata.mode));
            }
        }
    }
    pub async fn write_new(&self, name: &str, bytes: &[u8], mode: u32) -> Result<(), Error> {
        let path = self.path(name);
        // Empty files still require a create-new operation.
        for (offset, chunk) in bytes
            .chunks(1024 * 1024)
            .enumerate()
            .chain(bytes.is_empty().then_some((0, &[][..])))
        {
            self.invoke(Operation::Write(entries::WriteFile {
                path: path.clone(),
                offset: (offset * 1024 * 1024) as u64,
                bytes: chunk.to_vec(),
                truncate: false,
                create_new: offset == 0,
                mode: (offset * 1024 * 1024 + chunk.len() == bytes.len()).then_some(mode),
            }))
            .await?;
        }
        Ok(())
    }
    pub async fn remove(&self, name: &str) -> Result<(), Error> {
        self.invoke(Operation::Remove {
            path: self.path(name),
        })
        .await?;
        Ok(())
    }
    pub async fn remove_tree(&self, name: &str) -> Result<(), Error> {
        let mut stack = vec![(self.clone(), name.to_owned(), false)];
        let mut remaining = 2048;
        while let Some((parent, name, visited)) = stack.pop() {
            if visited {
                parent.remove(&name).await?;
                continue;
            }
            if remaining == 0 {
                return Err(Error::Invalid("Skill garbage exceeds limits".into()));
            }
            remaining -= 1;
            let directory = parent.open(&name).await?;
            stack.push((parent, name, true));
            for entry in directory.entries().await? {
                match entry.kind {
                    Kind::Directory => stack.push((directory.clone(), entry.name, false)),
                    _ => directory.remove(&entry.name).await?,
                }
            }
        }
        Ok(())
    }
    pub async fn sync(&self) -> Result<(), Error> {
        self.invoke(Operation::Sync {
            path: self.prefix.clone(),
        })
        .await?;
        Ok(())
    }
    pub async fn rename(&self, from: &str, target: &Self, to: &str) -> Result<(), Error> {
        if !Arc::ptr_eq(&self.root, &target.root) {
            return Err(Error::Invalid(
                "Publication crosses filesystem grants".into(),
            ));
        }
        self.invoke(Operation::Rename {
            from: self.path(from),
            to: target.path(to),
        })
        .await?;
        Ok(())
    }
}
impl From<entries::Error> for Error {
    fn from(error: entries::Error) -> Self {
        use entries::Error as File;
        match error {
            File::Invalid(message) => Self::Invalid(message),
            File::NotFound => Self::Io(std::io::ErrorKind::NotFound.into()),
            File::AlreadyExists => Self::Io(std::io::ErrorKind::AlreadyExists.into()),
            File::Retired | File::Cancelled => Self::Cancelled,
            File::Io(message) => Self::Io(std::io::Error::other(message)),
            File::OutcomeUnknown(message) => Self::OutcomeUnknown(message),
        }
    }
}
impl From<maka_runtime::tools::ToolError> for Error {
    fn from(error: maka_runtime::tools::ToolError) -> Self {
        use maka_runtime::tools::ToolError;
        match error {
            ToolError::Io { kind, message } => Self::Io(std::io::Error::new(kind, message)),
            ToolError::Failed(message) => Self::Invalid(message),
            error => Self::OutcomeUnknown(error.to_string()),
        }
    }
}
