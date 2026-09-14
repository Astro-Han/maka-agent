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

use super::selection::Selection;
use super::{SourceEvidence, invalid};
use crate::{StoreError, sequence_number};
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

/// The same v2 scoped digest as complete prefix(), without retaining source rows.
/// Even a single old giant observation is hashed in bounded SQL BLOB slices.
pub(super) async fn evidence(
    connection: &mut SqliteConnection,
    session: &str,
    through: u64,
) -> Result<SourceEvidence, StoreError> {
    selected(connection, &Selection::session(session), through).await
}

pub(super) async fn selected(
    connection: &mut SqliteConnection,
    selection: &Selection,
    through: u64,
) -> Result<SourceEvidence, StoreError> {
    let scope = selection.scope.clone();
    let session = selection
        .session
        .as_deref()
        .ok_or_else(|| invalid("context requires a Session"))?;
    let encoded = serde_json::to_vec(&scope)?;
    let mut digest = Sha256::new();
    digest.update(b"maka.log-prefix.v2\0");
    digest.update((encoded.len() as u64).to_be_bytes());
    digest.update(encoded);
    digest.update(through.to_be_bytes());
    let filter = Selection::predicate("runtime_events", "?4");
    let mut after = 0i64;
    loop {
        let mut oversized = None;
        {
            let mut rows = sqlx::query_as::<_, (i64, i64, Option<Vec<u8>>)>(sqlx::AssertSqlSafe(format!(
            "SELECT sequence, length(CAST(event_json AS BLOB)),
             CASE WHEN length(CAST(event_json AS BLOB)) <= 65536 THEN CAST(event_json AS BLOB) END FROM runtime_events
             WHERE sequence > ? AND sequence <= ?
             AND json_extract(event_json, '$.invocation.session_id') = ?3 AND {filter} ORDER BY sequence"
        )))
        .bind(after)
        .bind(through as i64)
        .bind(session)
        .bind(&selection.lineage)
        .fetch(&mut *connection);
            while let Some((sequence, bytes, payload)) = rows.try_next().await? {
                let bytes = sequence_number(bytes)?;
                digest.update(sequence_number(sequence)?.to_be_bytes());
                digest.update(bytes.to_be_bytes());
                after = sequence;
                if let Some(payload) = payload {
                    digest.update(payload);
                } else {
                    oversized = Some((sequence, bytes));
                    break;
                }
            }
        }
        let Some((sequence, bytes)) = oversized else {
            break;
        };
        let mut offset = 0u64;
        while offset < bytes {
            let slice: Vec<u8> = sqlx::query_scalar(
                "SELECT substr(CAST(event_json AS BLOB), ?, 65536) FROM runtime_events WHERE sequence = ?",
            ).bind((offset + 1) as i64).bind(sequence).fetch_one(&mut *connection).await?;
            if slice.is_empty() {
                return Err(invalid("source bytes disappeared"));
            }
            offset += slice.len() as u64;
            digest.update(slice);
        }
    }
    if sequence_number(after)? != through {
        return Err(invalid("coverage is not a selected source high-water"));
    }
    Ok(SourceEvidence {
        scope,
        high_water: through,
        digest: format!("sha256:{:x}", digest.finalize()),
    })
}

pub(super) async fn high_water(
    connection: &mut SqliteConnection,
    session: &str,
    before: i64,
) -> Result<u64, StoreError> {
    sequence_number(
        sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) FROM runtime_events WHERE sequence < ?
         AND json_extract(event_json, '$.invocation.session_id') = ?",
        )
        .bind(before)
        .bind(session)
        .fetch_one(connection)
        .await?,
    )
}
