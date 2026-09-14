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

use super::records::{advance, invalid, read_record, require_session};
use crate::StoreError;
use maka_runtime::{
    artifact::ArtifactSource,
    attachment::{AttachmentRef, MAX_ATTACHMENT_BYTES, StorageRef},
};
use sqlx::SqliteConnection;

/// Copy an explicitly authorized attachment without materializing its BLOB in the Host.
/// The caller commits this together with the message that owns the destination.
pub(crate) async fn copy_in_transaction(
    tx: &mut SqliteConnection,
    source: &AttachmentRef,
    destination: &AttachmentRef,
    turn: &str,
    created_at: u64,
) -> Result<(), StoreError> {
    let StorageRef::SessionFile {
        session_id: source_session,
        relative_path: source_id,
    } = &source.storage_ref
    else {
        return Err(invalid("Attachment source is not a Session Artifact"));
    };
    let StorageRef::SessionFile {
        session_id: target_session,
        relative_path: target_id,
    } = &destination.storage_ref
    else {
        return Err(invalid("Attachment destination is not a Session Artifact"));
    };
    let (mut artifact, digest) = read_record(tx, source_session, source_id)
        .await?
        .ok_or_else(|| invalid("Delegated attachment source no longer exists"))?;
    artifact.validate_attachment(source).map_err(invalid)?;
    if artifact.size_bytes > MAX_ATTACHMENT_BYTES
        || artifact.source == ArtifactSource::UserUpload
            && artifact.summary.as_ref() != Some(&digest)
    {
        return Err(StoreError::ArtifactConflict);
    }
    require_session(tx, target_session).await?;
    artifact.session_id = target_session.clone();
    artifact.id = target_id.clone();
    artifact.turn_id = turn.into();
    artifact.created_at = created_at;
    artifact.validate().map_err(invalid)?;
    artifact.validate_attachment(destination).map_err(invalid)?;
    let encoded = serde_json::to_string(&artifact)?;
    if encoded.len() > super::MAX_RECORD_BYTES {
        return Err(invalid("Copied attachment metadata exceeds limit"));
    }
    if let Some((existing, target_digest)) = read_record(tx, target_session, target_id).await? {
        if existing != artifact || target_digest != digest {
            return Err(StoreError::ArtifactConflict);
        }
        let same: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM artifacts s JOIN artifacts t
             ON t.session_id = ?3 AND t.id = ?4 WHERE s.session_id = ?1 AND s.id = ?2
             AND length(s.payload) = ?5 AND s.payload = t.payload)",
        )
        .bind(source_session)
        .bind(source_id)
        .bind(target_session)
        .bind(target_id)
        .bind(artifact.size_bytes as i64)
        .fetch_one(tx)
        .await?;
        return if same {
            Ok(())
        } else {
            Err(StoreError::ArtifactConflict)
        };
    }
    let inserted = sqlx::query(
        "INSERT INTO artifacts (session_id, id, created_at, record_json, content_sha256, payload)
         SELECT ?3, ?4, ?5, ?6, content_sha256, payload FROM artifacts
         WHERE session_id = ?1 AND id = ?2 AND length(payload) = ?7",
    )
    .bind(source_session)
    .bind(source_id)
    .bind(target_session)
    .bind(target_id)
    .bind(created_at as i64)
    .bind(encoded)
    .bind(artifact.size_bytes as i64)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted != 1 {
        return Err(StoreError::ArtifactConflict);
    }
    advance(tx, target_session).await
}
