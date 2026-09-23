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

use super::SessionRecord;
use crate::{
    EventLog, StoreError,
    context::{HistoryCut, history, safety},
    sequence_number,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::Connection;

/// Stable copy identity. Configuration is captured only on the first commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCopy {
    pub source_session_id: String,
    pub target_session_id: String,
    pub expected_source_revision: u64,
    pub cut: HistoryCut,
}

#[derive(Debug)]
pub enum SessionCopyResult<T> {
    Committed(Box<SessionRecord<T>>),
    SourceRevisionConflict { expected: u64, actual: u64 },
}

impl EventLog {
    /// Host-authorized history copy. The caller validates destination configuration
    /// against the captured source revision; no execution or workspace is cloned.
    /// Exact retries precede source CAS, including after a lost commit response.
    pub async fn copy_session<T: Serialize + DeserializeOwned + Send + 'static>(
        &self,
        request: SessionCopy,
        configuration: &T,
        now: u64,
    ) -> Result<SessionCopyResult<T>, StoreError> {
        self.validate_root()?;
        super::validate_id(&request.source_session_id)?;
        super::validate_id(&request.target_session_id)?;
        super::validate_time(now)?;
        if request.source_session_id == request.target_session_id
            || request.expected_source_revision == 0
            || request.expected_source_revision > super::MAX_SAFE_INTEGER
        {
            return Err(super::invalid("invalid Session copy identity or revision"));
        }
        if let HistoryCut::BeforeTurn(turn) | HistoryCut::ThroughTurn(turn) = &request.cut {
            super::validate_id(turn)?;
        }
        let configuration = serde_json::to_string(configuration)?;
        if configuration.len() > super::MAX_CONFIGURATION_BYTES {
            return Err(super::invalid("session configuration exceeds 64 KiB"));
        }
        let encoded = serde_json::to_string(&request)?;
        let fingerprint = maka_runtime::artifact::content_digest(encoded.as_bytes());
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let previous: Option<String> = sqlx::query_scalar(
                "SELECT request_json FROM session_history_copies WHERE session_id = ?",
            ).bind(&request.target_session_id).fetch_optional(&mut *tx).await?;
            if let Some(previous) = previous {
                if previous != encoded { return Err(StoreError::SessionConflict); }
                let session = super::read(&mut tx, &request.target_session_id).await?
                    .ok_or(StoreError::SessionNotFound)?;
                return Ok(SessionCopyResult::Committed(Box::new(session)));
            }
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM session_control WHERE id = ?)",
            ).bind(&request.target_session_id).fetch_one(&mut *tx).await?;
            if exists { return Err(StoreError::SessionConflict); }
            let revision: Option<i64> = sqlx::query_scalar(
                "SELECT revision FROM session_control WHERE id = ?",
            ).bind(&request.source_session_id).fetch_optional(&mut *tx).await?;
            let actual = sequence_number(revision.ok_or(StoreError::SessionNotFound)?)?;
            if actual != request.expected_source_revision {
                return Ok(SessionCopyResult::SourceRevisionConflict {
                    expected: request.expected_source_revision, actual,
                });
            }
            let managed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM plugin_sessions WHERE session_id = ? AND managed = 1)",
            ).bind(&request.source_session_id).fetch_one(&mut *tx).await?;
            if managed {
                return Err(super::invalid("managed Session history requires its owner's lifecycle"));
            }
            let observed: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0) FROM event_log")
                .fetch_one(&mut *tx).await?;
            let through = history::resolve_cut(&mut tx, &request.source_session_id, &request.cut, observed).await?;
            safety::require_safe_through(&mut tx, &request.source_session_id, None, through).await?;
            super::insert(&mut tx, &request.target_session_id, &fingerprint, &configuration, now).await?;
            sqlx::query("INSERT INTO session_history_copies VALUES (?, ?, ?, ?, ?, ?)")
                .bind(&request.target_session_id).bind(&request.source_session_id)
                .bind(actual as i64).bind(through as i64).bind(observed).bind(encoded)
                .execute(&mut *tx).await?;
            // Each inherited row carries its original archive visibility, not the
            // new parent's current view. Later source pruning cannot alter a copy.
            sqlx::query(
                "INSERT INTO session_history_members
                 SELECT ?1, e.sequence, MIN(e.archives_before, ?2),
                     (SELECT a.sequence FROM runtime_events a WHERE a.kind = 'tool_result_archived'
                      AND CAST(json_extract(a.event_json,'$.fact.placeholder.identity.runtime_event_id') AS TEXT) = e.event_id
                      AND a.sequence < MIN(e.archives_before, ?2))
                 FROM session_history_events e WHERE e.owner_session_id = ?3 AND e.sequence <= ?4",
            ).bind(&request.target_session_id).bind(observed.saturating_add(1))
                .bind(&request.source_session_id).bind(through as i64).execute(&mut *tx).await?;
            crate::artifacts::history::retain(&mut tx, &request.source_session_id, &request.target_session_id, now).await?;
            let session = super::read(&mut tx, &request.target_session_id).await?
                .ok_or(StoreError::SessionNotFound)?;
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            Ok(SessionCopyResult::Committed(Box::new(session)))
        })).await
    }
}
