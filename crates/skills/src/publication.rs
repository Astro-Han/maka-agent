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
use cap_std::fs::Dir;
use maka_runtime::artifact::content_digest;
use tokio_util::sync::CancellationToken;

mod directory;
mod io;
use directory::Directory;
mod journal;
mod tree;
mod user;
pub use tree::Tree;
pub(crate) use user::{UserFiles, UserStore};

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
    skills: Directory,
    transactions: Directory,
    _lock: std::sync::Arc<io::PublicationLock>,
}
impl Publisher {
    pub fn open(
        data: &Dir,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<Self, Error> {
        let lock = std::sync::Arc::new(io::lock(data)?);
        io::child(data, "skills")?;
        io::child(data, "transactions")?;
        let root = Directory::private(data.try_clone()?, cancellation.clone());
        Ok(Self {
            skills: root.at("skills"),
            transactions: root.at("transactions"),
            _lock: lock,
        })
    }
    pub async fn capture(
        &self,
        id: &str,
        cancellation: &CancellationToken,
    ) -> Result<Option<Tree>, Error> {
        validate_id(id)?;
        match self.skills.open(id).await {
            Ok(directory) => Ok(Some(Tree::read(&directory, cancellation).await?)),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Once intent is durable, cancellation cannot discard the publication.
    /// A failure after that cut is recoverable; Conflict proves no source edit was lost.
    pub async fn publish(
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
        self.recover().await?;
        let intent = journal::Intent {
            schema: 1,
            id: id.into(),
            expected: expected.map(Tree::manifest),
            next: next.map(Tree::manifest),
        };
        if self
            .capture(id, cancellation)
            .await?
            .map(|tree| tree.manifest())
            != intent.expected
        {
            return Err(Error::Conflict);
        }
        let bytes = serde_json::to_vec(&intent)?;
        if bytes.len() > 64 * 1024 {
            return Err(Error::Invalid("Skill manifest exceeds limit".into()));
        }
        let hash = content_digest(&bytes);
        let name = format!("tx-{}-{}", uuid::Uuid::new_v4(), &hash[7..]);
        self.transactions.create_dir(&name).await?;
        self.transactions.sync().await?;
        let transaction = self.transactions.open(&name).await?;
        if let Some(next) = next {
            let directory = transaction.child("next").await?;
            next.write(&directory).await?;
        }
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        transaction
            .write_new("intent.pending", &bytes, 0o600)
            .await?;
        // Rename can have happened even when synchronizing its directory fails.
        let result = async {
            transaction
                .rename("intent.pending", &transaction, "intent.json")
                .await?;
            journal::replay(&self.skills, &transaction, &intent, &hash).await
        }
        .await;
        drop(transaction);
        if matches!(result, Err(Error::Conflict)) {
            self.collect(&name)
                .await
                .map_err(|error| Error::OutcomeUnknown(error.to_string()))?;
            return Err(Error::Conflict);
        }
        result.map_err(|error: Error| Error::OutcomeUnknown(error.to_string()))?;
        self.collect(&name)
            .await
            .map_err(|error| Error::OutcomeUnknown(error.to_string()))
    }

    pub async fn recover(&self) -> Result<(), Error> {
        let mut names = Vec::new();
        for entry in self.transactions.entries().await? {
            if names.len() == 128 {
                return Err(Error::Invalid("Too many pending Skill publications".into()));
            }
            let name = entry.name;
            if !matches!(
                entry.kind,
                maka_plugins::filesystem::entries::Kind::Directory
            ) {
                return Err(Error::Invalid("Unsafe publication entry".into()));
            }
            validate_transaction(&name)?;
            names.push(name);
        }
        names.sort();
        for name in names {
            if name.starts_with("gc-") {
                self.transactions.remove_tree(&name).await?;
                self.transactions.sync().await?;
                continue;
            }
            let transaction = self.transactions.open(&name).await?;
            match transaction.read("intent.json", 64 * 1024).await {
                Ok((bytes, _)) => {
                    let hash = content_digest(&bytes);
                    if !name.ends_with(&hash[7..]) {
                        return Err(Error::Invalid("Publication intent digest mismatch".into()));
                    }
                    let intent: journal::Intent = serde_json::from_slice(&bytes)?;
                    intent.validate()?;
                    match journal::replay(&self.skills, &transaction, &intent, &hash).await {
                        Ok(()) | Err(Error::Conflict) => {}
                        Err(error) => return Err(error),
                    }
                }
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    for entry in transaction.entries().await? {
                        if !matches!(entry.name.as_str(), "next" | "intent.pending") {
                            return Err(Error::Invalid(
                                "Publication lost its intent after acquiring source data".into(),
                            ));
                        }
                    }
                }
                Err(error) => return Err(error),
            }
            drop(transaction);
            self.collect(&name).await?;
        }
        Ok(())
    }
    async fn collect(&self, name: &str) -> Result<(), Error> {
        let gc = format!("gc-{}", &name[3..]);
        self.transactions
            .rename(name, &self.transactions, &gc)
            .await?;
        // This directory is exclusively owned staging/retired data, not a source path.
        self.transactions.remove_tree(&gc).await?;
        self.transactions.sync().await?;
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
