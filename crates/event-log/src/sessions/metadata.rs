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

use super::{
    MAX_CONFIGURATION_BYTES, MAX_SAFE_INTEGER, SessionRecord, advance_catalog, invalid, read,
    validate_id, validate_time,
};
use crate::{EventLog, StoreError};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;

/// A metadata CAS is distinct from event-log append and delivery revisions.
pub enum SessionMutation<T> {
    Committed(SessionRecord<T>),
    RevisionConflict { expected: u64, actual: u64 },
}

impl EventLog {
    pub async fn set_session_archived<T: DeserializeOwned + Send + 'static>(
        &self,
        id: &str,
        archived: bool,
        now: u64,
    ) -> Result<SessionRecord<T>, StoreError> {
        self.validate_root()?;
        validate_id(id)?;
        validate_time(now)?;
        let id = id.to_owned();
        self.connection.run(move |connection| Box::pin(async move {
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
        let record: SessionRecord<T> = read(&mut tx, &id).await?.ok_or(StoreError::SessionNotFound)?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_events AS opening
             WHERE opening.kind = 'invocation_opened'
             AND json_extract(opening.event_json, '$.invocation.session_id') = ?
             AND NOT EXISTS(SELECT 1 FROM runtime_events AS terminal
                 WHERE terminal.invocation_id = opening.invocation_id AND terminal.kind = 'invocation_ended'))",
        ).bind(&id).fetch_one(&mut *tx).await?;
        if active {
            return Err(StoreError::SessionBusy);
        }
        if record.archived == archived {
            return Ok(record);
        }
        let changed = sqlx::query(
            "UPDATE session_control SET archived = ?, revision = revision + 1, updated_at = ? WHERE id = ? AND revision < ?",
        ).bind(archived).bind(now.max(record.updated_at) as i64).bind(&id)
            .bind(MAX_SAFE_INTEGER as i64).execute(&mut *tx).await?.rows_affected();
        if changed != 1 { return Err(invalid("session revision exhausted")); }
        advance_catalog(&mut tx).await?;
        let record = read(&mut tx, &id).await?.ok_or(StoreError::SessionNotFound)?;
        tx.commit().await.map_err(StoreError::CommitUnknown)?;
        Ok(record)
        })).await
    }

    /// Mutates validated control metadata, including while execution is active.
    /// The creation fingerprint and all event facts remain unchanged. Compare
    /// revision before invoking the policy closure, even for a semantic no-op.
    pub async fn update_session_metadata<T, F>(
        &self,
        id: &str,
        expected: u64,
        update: F,
    ) -> Result<SessionMutation<T>, StoreError>
    where
        T: Serialize + DeserializeOwned + Clone + PartialEq + Send + 'static,
        F: FnOnce(&mut T) -> Result<(), StoreError> + Send + 'static,
    {
        self.validate_root()?;
        validate_id(id)?;
        if expected == 0 || expected > MAX_SAFE_INTEGER {
            return Err(invalid("invalid expected Session revision"));
        }
        let id = id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let mut record: SessionRecord<T> = read(&mut tx, &id)
                        .await?
                        .ok_or(StoreError::SessionNotFound)?;
                    if record.revision != expected {
                        return Ok(SessionMutation::RevisionConflict {
                            expected,
                            actual: record.revision,
                        });
                    }
                    let previous = record.configuration.clone();
                    update(&mut record.configuration)?;
                    if record.configuration == previous {
                        return Ok(SessionMutation::Committed(record));
                    }
                    let configuration = serde_json::to_string(&record.configuration)?;
                    if configuration.len() > MAX_CONFIGURATION_BYTES {
                        return Err(invalid("session configuration exceeds 64 KiB"));
                    }
                    // Metadata is not activity: neither timestamps nor runtime_events move.
                    if sqlx::query(
                        "UPDATE session_control SET configuration = ?, revision = revision + 1
             WHERE id = ? AND revision = ? AND revision < ?",
                    )
                    .bind(configuration)
                    .bind(&id)
                    .bind(expected as i64)
                    .bind(MAX_SAFE_INTEGER as i64)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected()
                        != 1
                    {
                        return Err(invalid("session metadata revision exhausted"));
                    }
                    advance_catalog(&mut tx).await?;
                    let record = read(&mut tx, &id)
                        .await?
                        .ok_or(StoreError::SessionNotFound)?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    Ok(SessionMutation::Committed(record))
                })
            })
            .await
    }
}
