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

use crate::{StoreError, sequence_number};
use futures_util::TryStreamExt;
use maka_presentation::ProjectionError;
use maka_runtime::event::StoredEvent;
use sqlx::{Row, Sqlite, SqliteConnection, query::Query, sqlite::SqliteArguments};

pub(super) const MAX_EVENTS: usize = 10_000;
const MAX_BYTES: usize = 16 * 1024 * 1024;

pub(super) async fn fence(tx: &mut SqliteConnection, through: u64) -> Result<i64, StoreError> {
    let through = i64::try_from(through).map_err(|_| ProjectionError::OutOfRange)?;
    let high: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM event_log")
        .fetch_one(tx)
        .await?;
    if through > high {
        return Err(StoreError::InvalidTransition(
            "transcript fence exceeds committed log".into(),
        ));
    }
    Ok(through)
}

/// Select actual opening plus exactly one model step or invocation boundary.
/// No fabricated facts and no deserialization of earlier completed steps.
pub(super) async fn selected(
    tx: &mut SqliteConnection,
    invocation: &str,
    step: Option<&str>,
    through: i64,
    boundary: bool,
) -> Result<Vec<StoredEvent>, StoreError> {
    if boundary {
        let kind: String = sqlx::query_scalar(
            "SELECT kind FROM runtime_events WHERE sequence = ? AND invocation_id = ?",
        )
        .bind(through)
        .bind(invocation)
        .fetch_one(&mut *tx)
        .await?;
        if kind == "workhub_delegated" {
            return read(
                tx,
                sqlx::query(
                    "SELECT sequence, length(CAST(event_json AS BLOB)) FROM runtime_events
                 WHERE invocation_id = ?1 AND sequence <= ?2
                   AND (kind = 'invocation_opened' OR sequence = ?2) ORDER BY sequence",
                )
                .bind(invocation)
                .bind(through),
            )
            .await;
        }
        if matches!(
            kind.as_str(),
            "tool_dispatched" | "tool_rejected" | "tool_settled"
        ) {
            return super::tools::selected(tx, invocation, through).await;
        }
    }
    // A failed/cancelled terminal can seal a request without a separate model
    // interruption (for example after a model-observation commit failure).
    let unresolved = if step.is_none() {
        pending_step(tx, invocation, through).await?
    } else {
        None
    };
    let step = step.or(unresolved.as_deref());
    // Bound count before retrieving payloads, then bound cumulative UTF-8 bytes
    // from SQLite lengths before any event JSON is deserialized.
    let query = sqlx::query(
        "SELECT sequence, length(CAST(event_json AS BLOB)) FROM runtime_events
         WHERE invocation_id = ?1 AND sequence <= ?3 AND (
           kind = 'invocation_opened'
           OR (?4 AND sequence = ?3)
           OR (?2 IS NOT NULL AND (
             (kind = 'model_requested' AND operation_id = ?2)
             OR (kind = 'model_observed' AND json_extract(event_json, '$.fact.step_id') = ?2)
           )))
         ORDER BY sequence LIMIT ?5",
    )
    .bind(invocation)
    .bind(step)
    .bind(through)
    .bind(boundary)
    .bind((MAX_EVENTS + 1) as i64);
    read(tx, query).await
}

pub(super) async fn read(
    tx: &mut SqliteConnection,
    query: Query<'_, Sqlite, SqliteArguments>,
) -> Result<Vec<StoredEvent>, StoreError> {
    let mut ids = Vec::new();
    let mut bytes = 0usize;
    let mut records = query.fetch(&mut *tx);
    while let Some(row) = records.try_next().await? {
        let size =
            usize::try_from(row.try_get::<i64, _>(1)?).map_err(|_| StoreError::PrefixTooLarge)?;
        bytes = bytes.checked_add(size).ok_or(StoreError::PrefixTooLarge)?;
        if ids.len() >= MAX_EVENTS || bytes > MAX_BYTES {
            return Err(StoreError::PrefixTooLarge);
        }
        ids.push(row.try_get::<i64, _>(0)?);
    }
    drop(records);
    let mut events = Vec::with_capacity(ids.len());
    for sequence in ids {
        let json: String =
            sqlx::query_scalar("SELECT event_json FROM runtime_events WHERE sequence = ?")
                .bind(sequence)
                .fetch_one(&mut *tx)
                .await?;
        events.push(StoredEvent {
            sequence: sequence_number(sequence)?,
            event: serde_json::from_str(&json)?,
        });
    }
    Ok(events)
}

pub(super) async fn pending_step(
    tx: &mut SqliteConnection,
    invocation: &str,
    through: i64,
) -> Result<Option<String>, StoreError> {
    Ok(sqlx::query_scalar(
        "SELECT operation_id FROM runtime_events AS request
         WHERE invocation_id = ?1 AND sequence <= ?2 AND kind = 'model_requested'
           AND EXISTS(SELECT 1 FROM runtime_events opening
               WHERE opening.invocation_id = request.invocation_id AND opening.kind = 'invocation_opened'
               AND json_extract(opening.event_json, '$.fact.input.kind') IN ('message', 'continuation', 'handoff'))
           AND COALESCE(json_extract(request.event_json, '$.fact.purpose'), 'main') = 'main'
           AND NOT EXISTS(SELECT 1 FROM runtime_events AS boundary
               WHERE boundary.invocation_id = request.invocation_id
                 AND boundary.operation_id = request.operation_id
                 AND boundary.kind IN ('model_completed', 'model_interrupted')
                 AND boundary.sequence <= ?2)
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(invocation)
    .bind(through)
    .fetch_optional(tx)
    .await?)
}
