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

use super::MAX_RECORD_BYTES;
use crate::StoreError;
use maka_runtime::{
    artifact::{Artifact, content_digest},
    interaction::entity_id,
};
use sqlx::SqliteConnection;

pub(super) fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.to_owned())
}

pub(super) fn ids(session: &str, artifact: Option<&str>) -> Result<(), StoreError> {
    entity_id(session).map_err(invalid)?;
    if let Some(id) = artifact {
        entity_id(id).map_err(invalid)?;
    }
    Ok(())
}

pub(super) async fn require_session(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<(), StoreError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM session_control WHERE id = ?)")
            .bind(id)
            .fetch_one(connection)
            .await?;
    if !exists {
        return Err(StoreError::SessionNotFound);
    }
    Ok(())
}

pub(super) fn decode(
    raw: &str,
    session_id: &str,
    artifact_id: &str,
) -> Result<Artifact, StoreError> {
    if raw.len() > MAX_RECORD_BYTES {
        return Err(invalid("Stored artifact exceeds metadata limit"));
    }
    let artifact: Artifact = serde_json::from_str(raw)?;
    artifact.validate().map_err(invalid)?;
    if artifact.session_id != session_id || artifact.id != artifact_id {
        return Err(invalid("Stored artifact identity mismatch"));
    }
    Ok(artifact)
}

pub(super) async fn read_record(
    connection: &mut SqliteConnection,
    session_id: &str,
    artifact_id: &str,
) -> Result<Option<(Artifact, String)>, StoreError> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT record_json, content_sha256 FROM artifacts WHERE session_id = ? AND id = ?",
    )
    .bind(session_id)
    .bind(artifact_id)
    .fetch_optional(connection)
    .await?;
    row.map(|(raw, digest)| Ok((decode(&raw, session_id, artifact_id)?, digest)))
        .transpose()
}

pub(super) async fn revision(
    connection: &mut SqliteConnection,
    session_id: &str,
) -> Result<String, StoreError> {
    let counter: Option<i64> =
        sqlx::query_scalar("SELECT revision FROM artifact_catalog WHERE session_id = ?")
            .bind(session_id)
            .fetch_optional(connection)
            .await?;
    Ok(content_digest(
        format!("artifact-catalog:{session_id}:{}", counter.unwrap_or(0)).as_bytes(),
    ))
}

pub(super) async fn advance(
    connection: &mut SqliteConnection,
    session_id: &str,
) -> Result<(), StoreError> {
    let changed = sqlx::query(
        "INSERT INTO artifact_catalog (session_id, revision) VALUES (?, 1)
        ON CONFLICT(session_id) DO UPDATE SET revision = revision + 1 WHERE revision < 9007199254740991"
    ).bind(session_id).execute(connection).await?.rows_affected();
    if changed != 1 {
        return Err(invalid("Artifact revision exhausted"));
    }
    Ok(())
}
