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
    Inventory,
    closure::Closure,
    format::{Blob, CHUNK, MAX_BLOB_BYTES, Record, Writer},
};
use crate::{StoreError, sequence_number};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;
use tokio::io::AsyncWrite;

pub(super) async fn export<W: AsyncWrite + Unpin>(
    db: &mut SqliteConnection,
    inventory: &Inventory,
    closure: &Closure,
    writer: &mut Writer<W>,
) -> Result<(), StoreError> {
    for sequence in &closure.events {
        let tool: Option<(String, i64, Option<String>, Option<i64>)> = sqlx::query_as(
            "SELECT e.event_id,length(p.payload),json_extract(e.event_json,'$.fact.outcome.raw.digest'),
                json_extract(e.event_json,'$.fact.outcome.raw.bytes')
             FROM runtime_events e JOIN tool_result_payloads p ON p.event_id=e.event_id WHERE e.sequence=?"
        ).bind(*sequence as i64).fetch_optional(&mut *db).await?;
        if let Some((event_id, bytes, digest, expected_bytes)) = tool {
            if expected_bytes != Some(bytes) {
                return Err(invalid("tool payload length changed"));
            }
            let blob = Blob::ToolResult {
                event_id,
                bytes: sequence_number(bytes)?,
                digest: digest.ok_or_else(|| invalid("unbound tool payload"))?,
            };
            transfer(db, blob, writer).await?;
        }
        let composition: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT e.event_id,c.digest,length(c.surface) FROM runtime_events e
             JOIN model_request_compositions r ON r.event_id=e.event_id
             JOIN request_compositions c ON c.digest=r.digest WHERE e.sequence=?",
        )
        .bind(*sequence as i64)
        .fetch_optional(&mut *db)
        .await?;
        if let Some((event_id, digest, bytes)) = composition {
            transfer(
                db,
                Blob::Composition {
                    event_id,
                    bytes: sequence_number(bytes)?,
                    digest,
                },
                writer,
            )
            .await?;
        }
    }
    for session in &inventory.sessions {
        let mut after = None::<String>;
        loop {
            let next: Option<(String, String, String, i64)> = sqlx::query_as(
                "SELECT id,record_json,content_sha256,length(payload) FROM artifacts
                 WHERE session_id=?1 AND (?2 IS NULL OR id>?2) ORDER BY id LIMIT 1",
            )
            .bind(&session.id)
            .bind(&after)
            .fetch_optional(&mut *db)
            .await?;
            let Some((id, json, digest, bytes)) = next else {
                break;
            };
            let metadata: maka_runtime::artifact::Artifact = serde_json::from_str(&json)?;
            metadata.validate().map_err(invalid)?;
            if metadata.session_id != session.id
                || metadata.id != id
                || metadata.size_bytes != sequence_number(bytes)?
            {
                return Err(invalid("artifact metadata differs from stored bytes"));
            }
            transfer(db, Blob::Artifact { metadata, digest }, writer).await?;
            after = Some(id);
        }
        let mut after = None::<(String, String)>;
        loop {
            let next: Option<(String,String,String)> = sqlx::query_as(
                "SELECT source_session_id,source_artifact_id,artifact_id FROM session_history_artifacts
                 WHERE session_id=?1 AND (?2 IS NULL OR (source_session_id,source_artifact_id)>(?2,?3))
                 ORDER BY source_session_id,source_artifact_id LIMIT 1"
            ).bind(&session.id).bind(after.as_ref().map(|key| &key.0)).bind(after.as_ref().map(|key| &key.1))
                .fetch_optional(&mut *db).await?;
            let Some((source_session, source_artifact, artifact)) = next else {
                break;
            };
            after = Some((source_session.clone(), source_artifact.clone()));
            writer
                .record(&Record::HistoryArtifact {
                    session: session.id.clone(),
                    source_session,
                    source_artifact,
                    artifact,
                })
                .await?;
        }
    }
    Ok(())
}

async fn transfer<W: AsyncWrite + Unpin>(
    db: &mut SqliteConnection,
    blob: Blob,
    writer: &mut Writer<W>,
) -> Result<(), StoreError> {
    let length = blob.bytes();
    if length > MAX_BLOB_BYTES {
        return Err(StoreError::PrefixTooLarge);
    }
    let mut offset = 0;
    let mut hash = Sha256::new();
    writer.record(&Record::Blob(blob.clone())).await?;
    while offset < length {
        let size = (length - offset).min(CHUNK as u64) as i64;
        let start = (offset + 1) as i64;
        let bytes: Vec<u8> = match &blob {
            Blob::ToolResult { event_id, .. } => {
                sqlx::query_scalar(
                    "SELECT substr(payload,?2,?3) FROM tool_result_payloads WHERE event_id=?1",
                )
                .bind(event_id)
                .bind(start)
                .bind(size)
                .fetch_one(&mut *db)
                .await?
            }
            Blob::Composition { digest, .. } => {
                sqlx::query_scalar(
                    "SELECT substr(surface,?2,?3) FROM request_compositions WHERE digest=?1",
                )
                .bind(digest)
                .bind(start)
                .bind(size)
                .fetch_one(&mut *db)
                .await?
            }
            Blob::Artifact { metadata, .. } => {
                sqlx::query_scalar(
                    "SELECT substr(payload,?3,?4) FROM artifacts WHERE session_id=?1 AND id=?2",
                )
                .bind(&metadata.session_id)
                .bind(&metadata.id)
                .bind(start)
                .bind(size)
                .fetch_one(&mut *db)
                .await?
            }
        };
        if bytes.len() != size as usize {
            return Err(invalid("bundle payload disappeared"));
        }
        hash.update(&bytes);
        writer.bytes(&bytes).await?;
        offset += bytes.len() as u64;
    }
    if format!("sha256:{:x}", hash.finalize()) != blob.digest() {
        return Err(invalid("bundle payload digest mismatch"));
    }
    Ok(())
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
