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

//! Chunked import admission. Canonical rows are immutable immediately; only
//! publication creates the Session. Staging never rewrites an old log fence.
use super::{PluginSession, origin, validate_id, validate_time};
use crate::{EventLog, StoreError};
use maka_runtime::{
    artifact::content_digest,
    event::{Fact, Invocation, RuntimeEvent},
    import::{MAX_IMPORT_BYTES, MAX_IMPORT_RECORDS, Record, Source},
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, SqliteConnection};
use std::time::{Duration, UNIX_EPOCH};

pub use maka_runtime::import::{ImportProgress, ImportState};

impl EventLog {
    /// Frozen admission configuration, not permission to publish or execute.
    pub async fn session_import_configuration<T: DeserializeOwned + Send + 'static>(
        &self,
        session: &str,
    ) -> Result<T, StoreError> {
        self.validate_root()?;
        validate_id(session)?;
        let session = session.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let json: String = sqlx::query_scalar(
                        "SELECT configuration FROM session_imports WHERE session_id=?",
                    )
                    .bind(session)
                    .fetch_optional(connection)
                    .await?
                    .ok_or(StoreError::SessionNotFound)?;
                    Ok(serde_json::from_str(&json)?)
                })
            })
            .await
    }
    pub async fn begin_session_import<T: Serialize>(
        &self,
        owner: &PluginSession,
        source: &Source,
        configuration: &T,
        now: u64,
    ) -> Result<ImportProgress, StoreError> {
        self.validate_root()?;
        owner.validate()?;
        source.validate().map_err(invalid)?;
        validate_time(now)?;
        let configuration = serde_json::to_string(configuration)?;
        if configuration.len() > super::MAX_CONFIGURATION_BYTES {
            return Err(invalid("import configuration exceeds capacity"));
        }
        let source = serde_json::to_string(source)?;
        let owner = owner.clone();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let previous: Option<(String, String)> = sqlx::query_as(
                "SELECT source_json, configuration FROM session_imports WHERE session_id=?"
            ).bind(&owner.session_id).fetch_optional(&mut *tx).await?;
            if let Some(previous) = previous {
                origin::check(&mut tx, &owner.session_id, Some(&owner)).await?;
                if previous != (source, configuration) { return Err(StoreError::SessionConflict); }
                return progress(&mut tx, &owner.session_id).await;
            }
            let occupied: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM session_control WHERE id=?1
                    UNION ALL SELECT 1 FROM session_retirements WHERE session_id=?1
                    UNION ALL SELECT 1 FROM session_history_copies WHERE session_id=?1
                    UNION ALL SELECT 1 FROM event_log WHERE event_session=?1)"
            ).bind(&owner.session_id).fetch_one(&mut *tx).await?;
            if occupied { return Err(StoreError::SessionConflict); }
            origin::insert(&mut tx, &owner).await?;
            sqlx::query("INSERT INTO session_imports(session_id,source_json,configuration,created_at,state,records,bytes,conversation)
                VALUES(?,?,?,?,'collecting',0,0,0)")
                .bind(&owner.session_id).bind(source).bind(configuration).bind(now as i64)
                .execute(&mut *tx).await?;
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            Ok(ImportProgress { state: ImportState::Collecting, records: 0, bytes: 0 })
        })).await
    }

    pub async fn session_import_progress(
        &self,
        session: &str,
    ) -> Result<ImportProgress, StoreError> {
        self.validate_root()?;
        validate_id(session)?;
        let session = session.to_owned();
        self.connection
            .run(move |connection| Box::pin(async move { progress(connection, &session).await }))
            .await
    }

    /// One bounded writer job. Exact retry compares original canonical bytes,
    /// including after material reclamation. No missing or overlapping suffix.
    pub async fn append_session_import(
        &self,
        session: &str,
        position: u64,
        records: Vec<Record>,
    ) -> Result<ImportProgress, StoreError> {
        self.validate_root()?;
        validate_id(session)?;
        if records.is_empty() || records.len() > 8 || position >= MAX_IMPORT_RECORDS {
            return Err(invalid("invalid import batch"));
        }
        for record in &records {
            record.validate().map_err(invalid)?;
        }
        if serde_json::to_vec(&records)?.len() > maka_runtime::import::MAX_RECORD_BYTES {
            return Err(invalid("import batch exceeds its byte limit"));
        }
        let session = session.to_owned();
        let commits = self.commits.clone();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let mut current = progress(&mut tx, &session).await?;
            if current.state == ImportState::Abandoned { return Err(StoreError::SessionRetired); }
            let end = position + records.len() as u64;
            if position > current.records || (position < current.records && end > current.records) {
                return Err(invalid("import batch does not match its next position"));
            }
            let retry = end <= current.records;
            if !retry && (current.state != ImportState::Collecting || end > MAX_IMPORT_RECORDS) {
                return Err(invalid("import no longer accepts records"));
            }
            let (source, now): (String, i64) = sqlx::query_as(
                "SELECT source_json,created_at FROM session_imports WHERE session_id=?"
            ).bind(&session).fetch_one(&mut *tx).await?;
            let source: Source = serde_json::from_str(&source)?;
            let mut last = None;
            for (offset, record) in records.into_iter().enumerate() {
                let index = position + offset as u64;
                let event_id = identity(&session, "message", &index.to_string());
                let conversation = record.is_conversation();
                let event = RuntimeEvent {
                    id: event_id.clone(),
                    invocation: Invocation {
                        session_id: session.clone(),
                        turn_id: identity(&session, "turn", &record.source_turn_id),
                        run_id: identity(&session, "run", &index.to_string()),
                        invocation_id: identity(&session, "record", &index.to_string()),
                    },
                    recorded_at: UNIX_EPOCH + Duration::from_millis(now as u64),
                    fact: Fact::MessageImported { source: source.clone(), record: Box::new(record) },
                };
                let json = serde_json::to_string(&event)?;
                if retry {
                    let prior: (Option<String>, Option<String>) = sqlx::query_as(
                        "SELECT event_json,body_digest FROM event_log WHERE event_id=?"
                    ).bind(event_id).fetch_one(&mut *tx).await?;
                    let digest = prior.0.map(|body| content_digest(body.as_bytes())).or(prior.1);
                    if digest.as_deref() != Some(content_digest(json.as_bytes()).as_str()) {
                        return Err(StoreError::EventConflict);
                    }
                    continue;
                }
                current.bytes += json.len() as u64;
                if current.bytes > MAX_IMPORT_BYTES { return Err(invalid("import exceeds total byte capacity")); }
                let inserted = sqlx::query("INSERT INTO event_log(event_id,invocation_id,kind,event_json) VALUES(?,?,'message_imported',?)")
                    .bind(&event.id).bind(&event.invocation.invocation_id).bind(json).execute(&mut *tx).await?;
                last = Some(inserted.last_insert_rowid() as u64);
                super::project_execution(&mut tx, inserted.last_insert_rowid() as u64).await?;
                sqlx::query("UPDATE session_imports SET records=records+1,bytes=?,conversation=conversation+? WHERE session_id=?")
                    .bind(current.bytes as i64).bind(i64::from(conversation)).bind(&session).execute(&mut *tx).await?;
            }
            if retry { return Ok(current); }
            current.records = end;
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            if let Some(sequence) = last { commits.send_replace(sequence); }
            Ok(current)
        })).await
    }

    /// Publication and its receipt share Session creation's writer transaction.
    /// A caller may disappear after acceptance without hiding a committed Session.
    pub async fn publish_session_import(
        &self,
        session: &str,
        records: u64,
    ) -> Result<ImportProgress, StoreError> {
        self.validate_root()?;
        validate_id(session)?;
        let session = session.to_owned();
        let commits = self.commits.clone();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let mut current = progress(&mut tx, &session).await?;
            if current.records != records { return Err(invalid("import publication count changed")); }
            match current.state {
                ImportState::Published => return Ok(current),
                ImportState::Abandoned => return Err(StoreError::SessionRetired),
                ImportState::Collecting => {}
            }
            let (configuration, now, conversation, fingerprint): (String, i64, i64, String) = sqlx::query_as(
                "SELECT i.configuration,i.created_at,i.conversation,p.fingerprint FROM session_imports i
                 JOIN plugin_sessions p USING(session_id) WHERE i.session_id=?"
            ).bind(&session).fetch_one(&mut *tx).await?;
            if conversation == 0 { return Err(invalid("source has no importable conversation")); }
            let authority: Option<String> = sqlx::query_scalar(
                "SELECT authority_session_id FROM plugin_sessions WHERE session_id=?"
            ).bind(&session).fetch_one(&mut *tx).await?;
            origin::retain_authority(&mut tx, authority.as_deref()).await?;
            sqlx::query("UPDATE session_imports SET state='published' WHERE session_id=?")
                .bind(&session).execute(&mut *tx).await?;
            super::insert(&mut tx, &session, &fingerprint, &configuration, now as u64).await?;
            let through: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence),0) FROM event_log")
                .fetch_one(&mut *tx).await?;
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            commits.send_replace(through as u64);
            current.state = ImportState::Published;
            Ok(current)
        })).await
    }

    pub async fn abandon_session_import(
        &self,
        session: &str,
    ) -> Result<ImportProgress, StoreError> {
        self.validate_root()?;
        validate_id(session)?;
        let session = session.to_owned();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let mut current = progress(&mut tx, &session).await?;
            match current.state {
                ImportState::Published | ImportState::Abandoned => return Ok(current),
                ImportState::Collecting => {}
            }
            sqlx::query("UPDATE session_imports SET state='abandoned' WHERE session_id=?")
                .bind(&session).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO session_retirements(session_id,remove_session,completed) VALUES(?,1,1)")
                .bind(&session).execute(&mut *tx).await?;
            for statement in [
                "DELETE FROM catalog_messages WHERE session_id=?",
                "DELETE FROM transcript_rows WHERE session_id=?",
                "DELETE FROM transcript_text WHERE session_id=?",
                "DELETE FROM transcript_progress WHERE session_id=?",
            ] {
                sqlx::query(statement).bind(&session).execute(&mut *tx).await?;
            }
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            current.state = ImportState::Abandoned;
            Ok(current)
        })).await
    }
}

async fn progress(
    connection: &mut SqliteConnection,
    session: &str,
) -> Result<ImportProgress, StoreError> {
    let (state, records, bytes): (String, i64, i64) =
        sqlx::query_as("SELECT state,records,bytes FROM session_imports WHERE session_id=?")
            .bind(session)
            .fetch_optional(connection)
            .await?
            .ok_or(StoreError::SessionNotFound)?;
    Ok(ImportProgress {
        state: match state.as_str() {
            "collecting" => ImportState::Collecting,
            "published" => ImportState::Published,
            "abandoned" => ImportState::Abandoned,
            _ => return Err(invalid("invalid import state")),
        },
        records: crate::sequence_number(records)?,
        bytes: crate::sequence_number(bytes)?,
    })
}

fn identity(session: &str, kind: &str, key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for part in ["maka.import.v1", session, kind, key] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("import-{:x}", hash.finalize())
}

fn invalid(message: &'static str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
