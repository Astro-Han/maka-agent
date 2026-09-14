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

//! Separate prototype database; does not open or migrate old Maka State Roots.

mod append;
pub mod archive;
pub mod artifacts;
mod catalog_changes;
pub mod connection;
pub mod context;
mod continuation;
pub mod interactions;
pub mod message_admissions;
mod message_identity;
pub mod message_interrupts;
pub mod message_queue;
pub mod message_resolution;
pub mod message_sources;
pub mod observation;
mod prefix;
pub mod projects;
pub mod recovery;
pub mod root;
pub mod run_prefix;
mod schema;
pub mod sessions;
pub mod shell_runs;
mod sqlite_functions;
pub mod steering;
mod steering_delivery;
mod tool_calls;
mod tool_payloads;
pub mod transcript;
pub mod turns;
pub mod workhub;

use std::fs::OpenOptions;
use std::path::Path;
use std::sync::Arc;

use connection::{ConnectionAuthority, OwnedConnection};
use sqlx::sqlite::SqliteConnectOptions;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Project(#[from] projects::ProjectError),
    #[error(transparent)]
    Archive(#[from] archive::ArchiveError),
    #[error("event log already has a writer")]
    WriterBusy,
    #[error("not a supported Rust prototype event log; old Maka databases are not migrated")]
    UnsupportedDatabase,
    #[error("invocation is sealed")]
    Sealed,
    #[error("event identity conflicts with committed content")]
    EventConflict,
    #[error("session create request conflicts with committed configuration")]
    SessionConflict,
    #[error("artifact identity conflicts with committed metadata or payload")]
    ArtifactConflict,
    #[error("artifact chunk offset is out of range")]
    ArtifactOffset,
    #[error("shell resource identity already exists")]
    ShellConflict,
    #[error("shell resource not found in this session")]
    ShellNotFound,
    #[error("session not found")]
    SessionNotFound,
    #[error("session has an unsealed invocation")]
    SessionBusy,
    #[error("catalog revision changed from {expected} to {actual}")]
    RevisionConflict { expected: String, actual: String },
    #[error("invalid event transition: {0}")]
    InvalidTransition(String),
    #[error("prefix exceeds the requested event or byte limit")]
    PrefixTooLarge,
    #[error("derived transcript identity conflicts with its committed bytes")]
    TranscriptConflict,
    #[error(transparent)]
    Projection(#[from] maka_presentation::ProjectionError),
    #[error("event log mutex poisoned")]
    Poisoned,
    #[error("event commit outcome unknown: {0}")]
    CommitUnknown(sqlx::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("database initialization worker failed")]
    Initialization,
    #[error("database connection closed before accepting the operation")]
    ConnectionClosed,
    #[error("accepted database operation outcome is unknown")]
    OperationUnknown,
    #[error("database close failed: {0}")]
    CloseFailed(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub struct EventLog {
    // The independent owner drains SQLx and closes it before releasing its lease.
    connection: OwnedConnection,
    root_owner: Option<Arc<root::RootOwner>>,
    commits: tokio::sync::watch::Sender<u64>,
    shell_changes: tokio::sync::broadcast::Sender<shell_runs::ShellChange>,
}

impl EventLog {
    pub async fn for_root(owner: Arc<root::RootOwner>) -> Result<Self, StoreError> {
        owner.validate_current()?;
        let path = owner.canonical_path().join(root::ROOT_DATABASE);
        Self::open_owned(&path, Some(owner)).await
    }

    fn validate_root(&self) -> Result<(), StoreError> {
        if let Some(owner) = &self.root_owner {
            owner.validate_current()?;
        }
        Ok(())
    }

    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        Self::open_owned(path, None).await
    }

    async fn open_owned(
        path: &Path,
        root_owner: Option<Arc<root::RootOwner>>,
    ) -> Result<Self, StoreError> {
        // A symlink alias must share the same lease and SQLite sidecars.
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if path.symlink_metadata().is_ok() {
                    return Err(StoreError::InvalidTransition(
                        "dangling database symlink is unsupported".into(),
                    ));
                }
                let name = path.file_name().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "missing database filename",
                    )
                })?;
                let parent = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                parent.canonicalize()?.join(name)
            }
            Err(error) => return Err(error.into()),
        };
        // Different hard-link names cannot share WAL sidecars safely.
        #[cfg(unix)]
        if let Ok(metadata) = path.metadata() {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(StoreError::InvalidTransition(
                    "hard-linked event log is unsupported".into(),
                ));
            }
        }
        // A distinct stable lock file avoids colliding with SQLite's own
        // byte-range locks on the database. Never unlink it while in use.
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".writer.lock");
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        match lease.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(StoreError::WriterBusy),
            Err(std::fs::TryLockError::Error(error)) => return Err(StoreError::Io(error)),
        }
        let connection = OwnedConnection::open(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
            ConnectionAuthority::Writer {
                lease,
                root: root_owner.clone(),
            },
            |connection| Box::pin(schema::initialize_connection(connection)),
        )
        .await?;
        let high_water = connection
            .run(|connection| {
                Box::pin(async move {
                    sequence_number(
                        sqlx::query_scalar::<_, i64>(
                            "SELECT COALESCE(MAX(sequence), 0) FROM event_log",
                        )
                        .fetch_one(connection)
                        .await?,
                    )
                })
            })
            .await?;
        Ok(Self {
            connection,
            root_owner,
            commits: tokio::sync::watch::channel(high_water).0,
            shell_changes: tokio::sync::broadcast::channel(256).0,
        })
    }

    /// Finish every accepted operation and close SQLite before releasing its lease.
    pub async fn close(self) -> Result<(), StoreError> {
        self.connection.close().await
    }

    /// Close ingress and drain while existing observers still hold log handles.
    pub async fn shutdown(&self) -> Result<(), StoreError> {
        self.connection.shutdown().await
    }

    /// A coalescible wakeup, not a replacement for committed facts. Register
    /// before reading a bootstrap snapshot, then catch up from its SQL cursor.
    pub fn subscribe_commits(&self) -> tokio::sync::watch::Receiver<u64> {
        self.commits.subscribe()
    }
}

fn sequence_number(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value)
        .map_err(|_| StoreError::InvalidTransition("negative ledger sequence".into()))
}
