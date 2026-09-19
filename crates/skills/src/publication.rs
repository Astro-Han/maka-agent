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

//! Recoverable publication of complete Skill directories. Callers serialize this
//! domain store and run its bounded filesystem work off the async executor.
use cap_fs_ext::DirExt;
use cap_std::{ambient_authority, fs::Dir};
use maka_runtime::artifact::content_digest;
use std::path::Path;
use tokio_util::sync::CancellationToken;

mod io;
mod journal;
mod tree;
mod user;
pub use tree::Tree;
pub(crate) use user::UserStore;

pub(crate) fn read_import(path: &Path) -> Result<Vec<u8>, Error> {
    let parent = path
        .parent()
        .filter(|_| path.is_absolute())
        .ok_or_else(|| Error::Invalid("Import requires an absolute file path".into()))?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::Invalid("Missing import filename".into()))?;
    let directory = Dir::open_ambient_dir(parent, ambient_authority())?;
    io::read(&directory, Path::new(name), 1024 * 1024).map(|(bytes, _)| bytes)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Invalid Skill publication: {0}")]
    Invalid(String),
    #[error("Skill files changed before publication")]
    Conflict,
    #[error("Skill publication was cancelled before its durable intent")]
    Cancelled,
    #[error("Skill publication needs recovery: {0}")]
    OutcomeUnknown(String),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
}

pub struct Publisher {
    skills: Dir,
    transactions: Dir,
    _lock: std::sync::Arc<io::PublicationLock>,
}
impl Publisher {
    /// Recovery of the previous domain layout belongs here, not in Root or
    /// the plugin kernel. Leave the old directory intact after draining it.
    pub fn recover_legacy(&self, root: &Path) -> Result<(), Error> {
        let root = Dir::open_ambient_dir(root, ambient_authority())?;
        let transactions = match root.open_dir_nofollow("skill-transactions") {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        Self {
            skills: self.skills.try_clone()?,
            transactions,
            _lock: self._lock.clone(),
        }
        .recover()
    }

    pub fn open(root: &Path, data: &Dir) -> Result<Self, Error> {
        if !root.is_absolute() {
            return Err(Error::Invalid("Publication root must be absolute".into()));
        }
        let root = Dir::open_ambient_dir(root, ambient_authority())?;
        let lock = std::sync::Arc::new(io::lock(data)?);
        Ok(Self {
            skills: io::child(&root, "skills")?,
            transactions: io::child(data, "transactions")?,
            _lock: lock,
        })
    }
    pub fn capture(
        &self,
        id: &str,
        cancellation: &CancellationToken,
    ) -> Result<Option<Tree>, Error> {
        validate_id(id)?;
        match self.skills.open_dir_nofollow(id) {
            Ok(directory) => Ok(Some(Tree::read(&directory, cancellation)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Once intent is durable, cancellation cannot discard the publication.
    /// A failure after that cut is recoverable; Conflict proves no source edit was lost.
    pub fn publish(
        &self,
        id: &str,
        expected: Option<&Tree>,
        next: Option<&Tree>,
        cancellation: &CancellationToken,
    ) -> Result<(), Error> {
        validate_id(id)?;
        if expected.is_none() && next.is_none() {
            return Err(Error::Invalid("Empty publication".into()));
        }
        self.recover()?;
        let intent = journal::Intent {
            schema: 1,
            id: id.into(),
            expected: expected.map(Tree::manifest),
            next: next.map(Tree::manifest),
        };
        if self.capture(id, cancellation)?.map(|tree| tree.manifest()) != intent.expected {
            return Err(Error::Conflict);
        }
        let bytes = serde_json::to_vec(&intent)?;
        if bytes.len() > 64 * 1024 {
            return Err(Error::Invalid("Skill manifest exceeds limit".into()));
        }
        let hash = content_digest(&bytes);
        let name = format!("tx-{}-{}", uuid::Uuid::new_v4(), &hash[7..]);
        self.transactions.create_dir(&name)?;
        io::sync(&self.transactions)?;
        let transaction = self.transactions.open_dir_nofollow(&name)?;
        if let Some(next) = next {
            let directory = io::child(&transaction, "next")?;
            next.write(&directory)?;
        }
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        io::write_new(&transaction, Path::new("intent.pending"), &bytes, 0o600)?;
        // Rename can have happened even when synchronizing its directory fails.
        let result = (|| {
            io::rename(&transaction, "intent.pending", &transaction, "intent.json")?;
            journal::replay(&self.skills, &transaction, &intent, &hash)
        })();
        drop(transaction);
        if matches!(result, Err(Error::Conflict)) {
            self.collect(&name)
                .map_err(|error| Error::OutcomeUnknown(error.to_string()))?;
            return Err(Error::Conflict);
        }
        result.map_err(|error: Error| Error::OutcomeUnknown(error.to_string()))?;
        self.collect(&name)
            .map_err(|error| Error::OutcomeUnknown(error.to_string()))
    }

    pub fn recover(&self) -> Result<(), Error> {
        let mut names = Vec::new();
        for entry in self.transactions.entries()? {
            let entry = entry?;
            if names.len() == 128 {
                return Err(Error::Invalid("Too many pending Skill publications".into()));
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::Invalid("Non-UTF8 publication identity".into()))?;
            if !entry.file_type()?.is_dir() || entry.file_type()?.is_symlink() {
                return Err(Error::Invalid("Unsafe publication entry".into()));
            }
            validate_transaction(&name)?;
            names.push(name);
        }
        names.sort();
        for name in names {
            if name.starts_with("gc-") {
                self.transactions.remove_dir_all(&name)?;
                io::sync(&self.transactions)?;
                continue;
            }
            let transaction = self.transactions.open_dir_nofollow(&name)?;
            match io::read(&transaction, Path::new("intent.json"), 64 * 1024) {
                Ok((bytes, _)) => {
                    let hash = content_digest(&bytes);
                    if !name.ends_with(&hash[7..]) {
                        return Err(Error::Invalid("Publication intent digest mismatch".into()));
                    }
                    let intent: journal::Intent = serde_json::from_slice(&bytes)?;
                    intent.validate()?;
                    match journal::replay(&self.skills, &transaction, &intent, &hash) {
                        Ok(()) | Err(Error::Conflict) => {}
                        Err(error) => return Err(error),
                    }
                }
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    for entry in transaction.entries()? {
                        let entry = entry?;
                        if !matches!(entry.file_name().to_str(), Some("next" | "intent.pending")) {
                            return Err(Error::Invalid(
                                "Publication lost its intent after acquiring source data".into(),
                            ));
                        }
                    }
                }
                Err(error) => return Err(error),
            }
            drop(transaction);
            self.collect(&name)?;
        }
        Ok(())
    }
    fn collect(&self, name: &str) -> Result<(), Error> {
        let gc = format!("gc-{}", &name[3..]);
        io::rename(&self.transactions, name, &self.transactions, &gc)?;
        // This directory is exclusively owned staging/retired data, not a source path.
        self.transactions.remove_dir_all(&gc)?;
        io::sync(&self.transactions)?;
        Ok(())
    }
}
fn validate_id(id: &str) -> Result<(), Error> {
    if crate::safe_source_id(id) {
        Ok(())
    } else {
        Err(Error::Invalid("Unsafe Skill publication identity".into()))
    }
}
fn validate_transaction(name: &str) -> Result<(), Error> {
    let valid = name.is_ascii()
        && name.len() == 104
        && matches!(&name[..3], "tx-" | "gc-")
        && name.as_bytes()[39] == b'-'
        && uuid::Uuid::parse_str(&name[3..39]).is_ok()
        && name[40..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if valid {
        Ok(())
    } else {
        Err(Error::Invalid("Unknown publication directory".into()))
    }
}
