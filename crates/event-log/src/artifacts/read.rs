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

use super::*;
use records::{ids, revision};

impl EventLog {
    /// One read snapshot binds revision, ordering and total; no payload is read.
    pub async fn list_artifacts(
        &self,
        session_id: &str,
        offset: u64,
        limit: usize,
    ) -> Result<ArtifactPage, StoreError> {
        self.validate_root()?;
        ids(session_id, None)?;
        if limit == 0 || limit > 128 || offset > 9_007_199_254_740_991 {
            return Err(invalid("Invalid artifact page bounds"));
        }
        let session_id = session_id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    require_session(&mut tx, &session_id).await?;
                    let revision = revision(&mut tx, &session_id).await?;
                    let total: i64 =
                        sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE session_id = ?")
                            .bind(&session_id)
                            .fetch_one(&mut *tx)
                            .await?;
                    let rows: Vec<(String, String)> = sqlx::query_as(
                        "SELECT id, record_json FROM artifacts WHERE session_id = ?
                ORDER BY created_at DESC, id ASC LIMIT ? OFFSET ?",
                    )
                    .bind(&session_id)
                    .bind(limit as i64)
                    .bind(offset as i64)
                    .fetch_all(&mut *tx)
                    .await?;
                    let records = rows
                        .into_iter()
                        .map(|(id, raw)| decode(&raw, &session_id, &id))
                        .collect::<Result<_, _>>()?;
                    Ok(ArtifactPage {
                        revision,
                        records,
                        total: total as u64,
                    })
                })
            })
            .await
    }

    pub async fn get_artifact(
        &self,
        session_id: &str,
        artifact_id: &str,
    ) -> Result<ArtifactEntry, StoreError> {
        self.validate_root()?;
        ids(session_id, Some(artifact_id))?;
        let (session_id, artifact_id) = (session_id.to_owned(), artifact_id.to_owned());
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    require_session(&mut tx, &session_id).await?;
                    let revision = revision(&mut tx, &session_id).await?;
                    let record = read_record(&mut tx, &session_id, &artifact_id)
                        .await?
                        .map(|(record, _)| record);
                    Ok(ArtifactEntry { revision, record })
                })
            })
            .await
    }

    /// SQLite slices the BLOB before returning it, including for small previews
    /// of a large upload. A missing item is distinct from an invalid offset.
    pub async fn read_artifact_chunk(
        &self,
        session_id: &str,
        artifact_id: &str,
        offset: u64,
        limit: usize,
    ) -> Result<Option<ArtifactChunk>, StoreError> {
        self.validate_root()?;
        ids(session_id, Some(artifact_id))?;
        if limit == 0 || limit as u64 > MAX_ATTACHMENT_BYTES || offset > 9_007_199_254_740_991 {
            return Err(invalid("Invalid artifact read bounds"));
        }
        let (session_id, artifact_id) = (session_id.to_owned(), artifact_id.to_owned());
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin().await?;
            require_session(&mut tx, &session_id).await?;
            let row: Option<(i64, Vec<u8>)> = sqlx::query_as(
                "SELECT length(payload), substr(payload, ?, ?) FROM artifacts WHERE session_id = ? AND id = ?"
            ).bind(offset as i64 + 1).bind(limit as i64).bind(&session_id).bind(&artifact_id)
                .fetch_optional(&mut *tx).await?;
            row.map(|(total, bytes)| {
                if offset > total as u64 { return Err(StoreError::ArtifactOffset); }
                Ok(ArtifactChunk { total_bytes: total as u64, bytes })
            }).transpose()
        })).await
    }
}
