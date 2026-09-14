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

mod copy;
mod read;
pub(crate) use copy::copy_in_transaction;
mod records;
use crate::{EventLog, StoreError};
use maka_runtime::{
    artifact::{Artifact, ArtifactSource, content_digest},
    attachment::MAX_ATTACHMENT_BYTES,
};
use records::{advance, decode, invalid, read_record, require_session};
use sqlx::Connection;

const MAX_RECORD_BYTES: usize = 16 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ArtifactDeletion {
    Deleted,
    NotFound,
    Protected,
}

#[derive(Debug)]
pub struct ArtifactPage {
    pub revision: String,
    pub records: Vec<Artifact>,
    pub total: u64,
}

#[derive(Debug)]
pub struct ArtifactEntry {
    pub revision: String,
    pub record: Option<Artifact>,
}

#[derive(Debug)]
pub struct ArtifactChunk {
    pub total_bytes: u64,
    pub bytes: Vec<u8>,
}

impl EventLog {
    /// Immutable payload + metadata in one transaction. Exact retries retain the
    /// first creation time; all other metadata and payload bytes must agree.
    pub async fn commit_artifact(
        &self,
        artifact: Artifact,
        bytes: impl AsRef<[u8]> + Send + 'static,
    ) -> Result<Artifact, StoreError> {
        self.validate_root()?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    // The job retains the payload owner, including any memory reservation,
                    // independently of its caller. Hashing runs on the database thread.
                    let artifact = commit_in_transaction(&mut tx, artifact, bytes.as_ref()).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    Ok(artifact)
                })
            })
            .await
    }

    /// Runtime-owned evidence cannot be deleted independently of its workflow.
    pub async fn delete_user_artifact(
        &self,
        session_id: &str,
        artifact_id: &str,
    ) -> Result<ArtifactDeletion, StoreError> {
        self.validate_root()?;
        records::ids(session_id, Some(artifact_id))?;
        let (session_id, artifact_id) = (session_id.to_owned(), artifact_id.to_owned());
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    require_session(&mut tx, &session_id).await?;
                    let Some((artifact, _)) =
                        read_record(&mut tx, &session_id, &artifact_id).await?
                    else {
                        return Ok(ArtifactDeletion::NotFound);
                    };
                    if !artifact.source.user_deletable() {
                        return Ok(ArtifactDeletion::Protected);
                    }
                    sqlx::query("DELETE FROM artifacts WHERE session_id = ? AND id = ?")
                        .bind(&session_id)
                        .bind(&artifact_id)
                        .execute(&mut *tx)
                        .await?;
                    advance(&mut tx, &session_id).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    Ok(ArtifactDeletion::Deleted)
                })
            })
            .await
    }
}

/// Shared immutable artifact write; the caller owns the enclosing transaction.
pub(crate) async fn commit_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    mut artifact: Artifact,
    bytes: &[u8],
) -> Result<Artifact, StoreError> {
    artifact.validate().map_err(invalid)?;
    if artifact.size_bytes != bytes.len() as u64 || artifact.size_bytes > MAX_ATTACHMENT_BYTES {
        return Err(invalid("Artifact payload size mismatch or limit exceeded"));
    }
    let encoded = serde_json::to_string(&artifact)?;
    if encoded.len() > MAX_RECORD_BYTES {
        return Err(invalid("Artifact metadata exceeds limit"));
    }
    let digest = content_digest(bytes);
    if artifact.source == ArtifactSource::UserUpload && artifact.summary.as_ref() != Some(&digest) {
        return Err(StoreError::ArtifactConflict);
    }
    require_session(connection, &artifact.session_id).await?;
    if let Some((existing, stored_digest)) =
        read_record(connection, &artifact.session_id, &artifact.id).await?
    {
        if artifact.source == ArtifactSource::ToolResultProjection
            && artifact.created_at != existing.created_at
        {
            return Err(StoreError::ArtifactConflict);
        }
        artifact.created_at = existing.created_at;
        if artifact != existing || digest != stored_digest {
            return Err(StoreError::ArtifactConflict);
        }
        verify_payload(connection, &artifact, bytes).await?;
        return Ok(existing);
    }
    sqlx::query(
        "INSERT INTO artifacts (session_id, id, created_at, record_json, content_sha256, payload)
        VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&artifact.session_id)
    .bind(&artifact.id)
    .bind(artifact.created_at as i64)
    .bind(encoded)
    .bind(digest)
    .bind(bytes)
    .execute(&mut *connection)
    .await?;
    advance(connection, &artifact.session_id).await?;
    Ok(artifact)
}

pub(crate) async fn verify_projection_replay(
    connection: &mut sqlx::SqliteConnection,
    artifact: &Artifact,
    bytes: &[u8],
) -> Result<(), StoreError> {
    let Some((existing, digest)) =
        read_record(connection, &artifact.session_id, &artifact.id).await?
    else {
        return Err(StoreError::EventConflict);
    };
    if &existing != artifact || digest != content_digest(bytes) {
        return Err(StoreError::EventConflict);
    }
    verify_payload(connection, artifact, bytes).await
}

async fn verify_payload(
    connection: &mut sqlx::SqliteConnection,
    artifact: &Artifact,
    bytes: &[u8],
) -> Result<(), StoreError> {
    let matches: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM artifacts WHERE session_id = ? AND id = ? AND payload = ?)",
    )
    .bind(&artifact.session_id)
    .bind(&artifact.id)
    .bind(bytes)
    .fetch_one(connection)
    .await?;
    if !matches {
        return Err(StoreError::ArtifactConflict);
    }
    Ok(())
}
