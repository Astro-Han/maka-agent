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

use super::{Error, directory::Directory};
use maka_plugins::filesystem::entries::Kind;
use maka_runtime::artifact::content_digest;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};
use tokio_util::sync::CancellationToken;

pub const MAX_TREE_BYTES: usize = 16 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;

#[derive(Clone)]
pub struct Tree {
    entries: BTreeMap<String, Entry>,
}
#[derive(Clone)]
enum Entry {
    Directory,
    File { bytes: Vec<u8>, mode: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Fact {
    Directory,
    File { hash: String, mode: u32 },
}
pub(super) type Manifest = BTreeMap<String, Fact>;

impl Tree {
    pub fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Only ordinary files and directories are admitted. Captured handles refuse links.
    pub(super) async fn read(
        directory: &Directory,
        cancellation: &CancellationToken,
    ) -> Result<Self, Error> {
        let mut result = Self::empty();
        let mut budget = MAX_TREE_BYTES;
        result
            .collect(directory, "", &mut budget, cancellation)
            .await?;
        Ok(result)
    }
    pub fn insert(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), Error> {
        validate(path)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(Error::Invalid("Skill resource exceeds file limit".into()));
        }
        let mut parent = Path::new(path).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            let name = path
                .to_str()
                .ok_or_else(|| Error::Invalid("Non-UTF8 Skill path".into()))?;
            match self.entries.get(name) {
                Some(Entry::File { .. }) => {
                    return Err(Error::Invalid(
                        "File shadows Skill resource directory".into(),
                    ));
                }
                Some(Entry::Directory) => {}
                None => {
                    self.entries.insert(name.into(), Entry::Directory);
                }
            }
            parent = path.parent();
        }
        if matches!(self.entries.get(path), Some(Entry::Directory)) {
            return Err(Error::Invalid("Skill artifact is a directory".into()));
        }
        let mode = match self.entries.get(path) {
            Some(Entry::File { mode, .. }) => *mode,
            _ => 0o600,
        };
        let old_size = self.bytes();
        let replaced = match self.entries.get(path) {
            Some(Entry::File { bytes, .. }) => bytes.len(),
            _ => 0,
        };
        if self.entries.len() >= MAX_ENTRIES && !self.entries.contains_key(path)
            || old_size - replaced + bytes.len() > MAX_TREE_BYTES
        {
            return Err(Error::Invalid("Skill resource tree exceeds limits".into()));
        }
        self.entries
            .insert(path.into(), Entry::File { bytes, mode });
        Ok(())
    }
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        match self.entries.get(path) {
            Some(Entry::File { bytes, .. }) => Some(bytes),
            _ => None,
        }
    }
    pub(super) fn manifest(&self) -> Manifest {
        self.entries
            .iter()
            .map(|(path, entry)| {
                (
                    path.clone(),
                    match entry {
                        Entry::Directory => Fact::Directory,
                        Entry::File { bytes, mode } => Fact::File {
                            hash: content_digest(bytes),
                            mode: *mode,
                        },
                    },
                )
            })
            .collect()
    }
    pub(super) async fn write(&self, directory: &Directory) -> Result<(), Error> {
        for (path, entry) in &self.entries {
            match entry {
                Entry::Directory => directory.create_dir(path).await?,
                Entry::File { bytes, mode } => directory.write_new(path, bytes, *mode).await?,
            }
        }
        // Reconfirm descendants before their parents during publication.
        for (path, entry) in self.entries.iter().rev() {
            if matches!(entry, Entry::Directory) {
                directory.open(path).await?.sync().await?;
            }
        }
        directory.sync().await
    }
    fn bytes(&self) -> usize {
        self.entries
            .values()
            .map(|entry| match entry {
                Entry::File { bytes, .. } => bytes.len(),
                _ => 0,
            })
            .sum()
    }
    async fn collect(
        &mut self,
        directory: &Directory,
        prefix: &str,
        budget: &mut usize,
        cancellation: &CancellationToken,
    ) -> Result<(), Error> {
        for entry in directory.entries().await? {
            if cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if self.entries.len() == MAX_ENTRIES {
                return Err(Error::Invalid("Too many Skill resources".into()));
            }
            let name = entry.name.as_str();
            let path = if prefix.is_empty() {
                name.into()
            } else {
                format!("{prefix}/{name}")
            };
            validate(&path)?;
            let kind = entry.kind;
            if matches!(kind, Kind::Directory) {
                let child = directory.open(name).await?;
                self.entries.insert(path.clone(), Entry::Directory);
                Box::pin(self.collect(&child, &path, budget, cancellation)).await?;
            } else if matches!(kind, Kind::File) {
                let (bytes, mode) = directory.read(name, MAX_FILE_BYTES.min(*budget)).await?;
                *budget -= bytes.len();
                self.entries.insert(path, Entry::File { bytes, mode });
            } else {
                return Err(Error::Invalid(
                    "Skill resources contain a link or non-regular file".into(),
                ));
            }
        }
        Ok(())
    }
}
pub(super) fn validate(path: &str) -> Result<(), Error> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(path).components().count() > 16
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::Invalid("Invalid Skill resource path".into()));
    }
    Ok(())
}
