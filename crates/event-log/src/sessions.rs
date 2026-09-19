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

//! Durable session control metadata. Execution history remains in runtime_events.

mod activity;
mod execution;
mod manager;
mod metadata;
pub use manager::ManagedSession;
pub(crate) mod read_state;
pub(crate) use activity::{initialize_execution, project_execution, register_functions};
pub(crate) use execution::advance_revision;
pub use execution::{CatalogMessage, SessionExecution, SessionExecutionState};
pub use metadata::SessionMutation;
pub use read_state::SessionReadState;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row, SqliteConnection};

use crate::{EventLog, StoreError};

pub const MAX_CONFIGURATION_BYTES: usize = 64 * 1024;
pub const MAX_SESSION_PAGE: usize = 32;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SessionRecord<T> {
    pub id: String,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub archived: bool,
    pub configuration: T,
    /// Exact stored configuration basis, independent of execution-driven revision.
    #[serde(skip)]
    pub configuration_digest: String,
    #[serde(default)]
    pub read_state: SessionReadState,
    /// Bounded execution projection read in the same snapshot as metadata.
    #[serde(skip)]
    pub execution: Option<SessionExecution>,
    /// Oldest unresolved interaction, read from canonical facts in the same snapshot.
    #[serde(skip)]
    pub pending_interaction_since: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct SessionPage<T> {
    pub revision: String,
    pub sessions: Vec<SessionRecord<T>>,
    pub next_cursor: Option<String>,
}

impl EventLog {
    pub async fn probe_session_create<T: DeserializeOwned + Send + 'static>(
        &self,
        id: &str,
        fingerprint: &str,
    ) -> Result<Option<SessionRecord<T>>, StoreError> {
        self.validate_root()?;
        validate_id(id)?;
        let (id, fingerprint) = (id.to_owned(), fingerprint.to_owned());
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    probe(&mut tx, &id, &fingerprint).await
                })
            })
            .await
    }

    /// Configuration is opaque here; callers must validate its domain semantics.
    pub async fn create_session<T: Serialize + DeserializeOwned + Send + 'static>(
        &self,
        id: &str,
        fingerprint: &str,
        configuration: &T,
        now: u64,
    ) -> Result<SessionRecord<T>, StoreError> {
        self.validate_root()?;
        validate_id(id)?;
        validate_time(now)?;
        let configuration = serde_json::to_string(configuration)?;
        if configuration.len() > MAX_CONFIGURATION_BYTES {
            return Err(invalid("session configuration exceeds 64 KiB"));
        }
        let (id, fingerprint) = (id.to_owned(), fingerprint.to_owned());
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    if let Some(record) = probe(&mut tx, &id, &fingerprint).await? {
                        return Ok(record);
                    }
                    insert(&mut tx, &id, &fingerprint, &configuration, now).await?;
                    let record = read(&mut tx, &id)
                        .await?
                        .ok_or(StoreError::SessionNotFound)?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    Ok(record)
                })
            })
            .await
    }

    pub async fn get_session<T: DeserializeOwned + Send + 'static>(
        &self,
        id: &str,
    ) -> Result<Option<SessionRecord<T>>, StoreError> {
        self.validate_root()?;
        validate_id(id)?;
        let id = id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    read(&mut tx, &id).await
                })
            })
            .await
    }

    /// A continuation requires the revision returned with its first page.
    pub async fn list_sessions<T: DeserializeOwned + Send + 'static>(
        &self,
        expected_revision: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<SessionPage<T>, StoreError> {
        self.validate_root()?;
        if limit == 0 || limit > MAX_SESSION_PAGE {
            return Err(invalid("session page limit must be 1..=32"));
        }
        if let Some(cursor) = cursor {
            validate_id(cursor)?;
            if expected_revision.is_none() {
                return Err(invalid("session continuation requires catalog revision"));
            }
        }
        let expected_revision = expected_revision.map(str::to_owned);
        let cursor = cursor.map(str::to_owned);
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let counter: i64 = sqlx::query_scalar(
                        "SELECT revision FROM session_catalog_revision WHERE singleton = 1",
                    )
                    .fetch_one(&mut *tx)
                    .await?;
                    let revision = format!(
                        "sha256:{:x}",
                        Sha256::digest(format!("session-catalog:{counter}"))
                    );
                    if let Some(expected) = expected_revision
                        && expected != revision
                    {
                        return Err(StoreError::RevisionConflict {
                            expected,
                            actual: revision,
                        });
                    }
                    let ids = sqlx::query_scalar::<_, String>(
                        "SELECT id FROM session_control WHERE id > ? ORDER BY id LIMIT ?",
                    )
                    .bind(cursor.as_deref().unwrap_or(""))
                    .bind((limit + 1) as i64)
                    .fetch_all(&mut *tx)
                    .await?;
                    let has_more = ids.len() > limit;
                    let mut sessions: Vec<SessionRecord<T>> = Vec::with_capacity(limit);
                    for id in ids.iter().take(limit) {
                        sessions.push(
                            read(&mut tx, id)
                                .await?
                                .ok_or(StoreError::SessionNotFound)?,
                        );
                    }
                    let next_cursor = if has_more {
                        sessions.last().map(|record| record.id.clone())
                    } else {
                        None
                    };
                    Ok(SessionPage {
                        revision,
                        sessions,
                        next_cursor,
                    })
                })
            })
            .await
    }
}

pub(crate) async fn insert(
    tx: &mut SqliteConnection,
    id: &str,
    fingerprint: &str,
    configuration: &str,
    now: u64,
) -> Result<(), StoreError> {
    validate_id(id)?;
    validate_time(now)?;
    if configuration.len() > MAX_CONFIGURATION_BYTES {
        return Err(invalid("session configuration exceeds 64 KiB"));
    }
    let reserved: Option<String> =
        sqlx::query_scalar("SELECT fingerprint FROM session_managers WHERE session_id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if reserved.is_some_and(|reserved| reserved != fingerprint) {
        return Err(StoreError::SessionConflict);
    }
    let inserted = sqlx::query(
        "INSERT INTO session_control SELECT ?, ?, 1, ?, ?, 0, ?
         WHERE NOT EXISTS (SELECT 1 FROM session_control WHERE id = ?)",
    )
    .bind(id)
    .bind(fingerprint)
    .bind(now as i64)
    .bind(now as i64)
    .bind(configuration)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(StoreError::SessionConflict);
    }
    advance_catalog(tx).await
}

pub(crate) async fn read<T: DeserializeOwned>(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<Option<SessionRecord<T>>, StoreError> {
    let raw = sqlx::query(
        "SELECT id, revision, created_at, updated_at, archived, configuration FROM session_control WHERE id = ?",
    ).bind(id).fetch_optional(&mut *connection).await?;
    if let Some(row) = raw {
        let configuration: String = row.try_get(5)?;
        if configuration.len() > MAX_CONFIGURATION_BYTES {
            return Err(invalid("oversized stored session configuration"));
        }
        let configuration_digest = maka_runtime::artifact::content_digest(configuration.as_bytes());
        let configuration = serde_json::from_str(&configuration)?;
        let execution = execution::read(connection, id).await?;
        let read_state = read_state::read(connection, id).await?;
        let pending_since: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(created_at) FROM interaction_requests request
             WHERE session_id = ? AND NOT EXISTS (
                 SELECT 1 FROM interaction_outcomes outcome
                 WHERE outcome.request_id = request.request_id)",
        )
        .bind(id)
        .fetch_one(&mut *connection)
        .await?;
        let pending_interaction_since = pending_since
            .map(|time| u64::try_from(time).map_err(|_| invalid("invalid interaction timestamp")))
            .transpose()?;
        Ok(Some(SessionRecord {
            id: row.try_get(0)?,
            revision: number(&row, 1)?,
            created_at: number(&row, 2)?,
            updated_at: number(&row, 3)?,
            archived: row.try_get(4)?,
            configuration,
            configuration_digest,
            execution,
            read_state,
            pending_interaction_since,
        }))
    } else {
        Ok(None)
    }
}

pub(crate) async fn advance_catalog(connection: &mut SqliteConnection) -> Result<(), StoreError> {
    if sqlx::query(
        "UPDATE session_catalog_revision SET revision = revision + 1 WHERE singleton = 1 AND revision < ?",
    ).bind(MAX_SAFE_INTEGER as i64).execute(connection).await?.rows_affected() != 1 {
        return Err(invalid("session catalog revision exhausted"));
    }
    Ok(())
}

pub(crate) fn validate_id(id: &str) -> Result<(), StoreError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid("invalid session ID"));
    }
    Ok(())
}

pub(crate) fn validate_time(now: u64) -> Result<(), StoreError> {
    if now > MAX_SAFE_INTEGER {
        return Err(invalid("timestamp exceeds safe integer range"));
    }
    Ok(())
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}

fn number(row: &sqlx::sqlite::SqliteRow, column: usize) -> Result<u64, StoreError> {
    let value: i64 = row.try_get(column)?;
    if value < 0 || value > MAX_SAFE_INTEGER as i64 {
        return Err(invalid("stored session number exceeds safe integer range"));
    }
    Ok(value as u64)
}

async fn probe<T: DeserializeOwned>(
    connection: &mut SqliteConnection,
    id: &str,
    fingerprint: &str,
) -> Result<Option<SessionRecord<T>>, StoreError> {
    if fingerprint.is_empty() || fingerprint.len() > 512 {
        return Err(invalid("invalid session fingerprint length"));
    }
    let previous: Option<String> =
        sqlx::query_scalar("SELECT fingerprint FROM session_control WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *connection)
            .await?;
    if previous.is_some_and(|previous| previous != fingerprint) {
        return Err(StoreError::SessionConflict);
    }
    read(connection, id).await
}
