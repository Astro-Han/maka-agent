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

//! Disposable message index; authority stays in the owner-typed canonical ledger.
mod active;
pub(crate) use active::interrupted_messages;
mod evidence;
pub mod navigation;
mod read;
mod tools;
mod workhub;
use crate::{EventLog, StoreError};
use maka_presentation::{InvocationView, MAX_TOOL_ROW_BYTES, ProjectionError, Row, watermark};
use maka_runtime::event::{Fact, ToolOutcome};
pub use read::*;
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row as SqlRow, SqliteConnection};

const MAX_BOUNDARIES: usize = 32;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;

pub(crate) async fn initialize(connection: &mut SqliteConnection) -> Result<(), StoreError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS transcript_rows (
            sequence INTEGER PRIMARY KEY CHECK(sequence >= 0),
            session_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            payload BLOB NOT NULL,
            digest TEXT NOT NULL,
            total_bytes INTEGER NOT NULL CHECK(total_bytes > 0),
            UNIQUE(session_id, message_id)
        )",
    )
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS transcript_session_sequence
            ON transcript_rows(session_id, sequence)",
    )
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS transcript_session_turn
            ON transcript_rows(session_id, turn_id, sequence)",
    )
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS transcript_progress (
            session_id TEXT PRIMARY KEY,
            through_sequence INTEGER NOT NULL CHECK(through_sequence >= 0)
        );",
    )
    .execute(&mut *connection)
    .await?;
    navigation::initialize(connection).await?;
    Ok(())
}

impl EventLog {
    /// Prepare at most 32 immutable boundaries through a fixed raw-log fence.
    /// False means another bounded call is needed. Oversize/unsupported evidence
    /// is an error, never an indefinitely preparing state. No commit notice is
    /// emitted: these writes carry no execution facts.
    pub async fn prepare_transcript(
        &self,
        session: &str,
        through: u64,
        max_boundaries: usize,
    ) -> Result<bool, StoreError> {
        crate::sessions::validate_id(session)?;
        if !(1..=MAX_BOUNDARIES).contains(&max_boundaries) {
            return Err(StoreError::InvalidTransition(
                "invalid transcript boundary limit".into(),
            ));
        }
        watermark(through)?;
        self.validate_root()?;
        let session = session.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let fence = evidence::fence(&mut tx, through).await?;
                    let progress: Option<i64> = sqlx::query_scalar(
                        "SELECT through_sequence FROM transcript_progress WHERE session_id = ?",
                    )
                    .bind(&session)
                    .fetch_optional(&mut *tx)
                    .await?;
                    if progress.is_some_and(|progress| progress >= fence) {
                        return Ok(true);
                    }
                    let progress = progress.unwrap_or(0);
                    // Read only small boundary headers here. Canonical payloads are selected
                    // and budget-checked separately, one boundary at a time.
                    let boundaries = {
                        let records = sqlx::query(
                            "SELECT sequence, invocation_id, operation_id FROM (
                             SELECT sequence, invocation_id, operation_id FROM runtime_events
                 WHERE json_extract(event_json, '$.invocation.session_id') = ?1
                   AND sequence > ?2 AND sequence <= ?3
                   AND NOT EXISTS (SELECT 1 FROM runtime_events opening
                       WHERE opening.invocation_id = runtime_events.invocation_id
                         AND opening.kind = 'invocation_opened'
                         AND json_extract(opening.event_json, '$.fact.input.kind') = 'context_compact')
                   AND (kind NOT IN ('model_completed', 'model_interrupted') OR
                       EXISTS (SELECT 1 FROM runtime_events request
             JOIN runtime_events opening ON opening.invocation_id = request.invocation_id
             AND opening.kind = 'invocation_opened'
             WHERE request.kind = 'model_requested' AND request.invocation_id = runtime_events.invocation_id
             AND request.operation_id = runtime_events.operation_id
             AND json_extract(opening.event_json, '$.fact.input.kind') IN ('message', 'continuation', 'handoff')
             AND json_extract(request.event_json, '$.fact.purpose') = 'main'))
                   AND (kind IN ('invocation_opened', 'message_steered', 'model_completed',
                                'model_interrupted', 'invocation_ended',
                                'tool_dispatched', 'tool_rejected', 'tool_settled', 'workhub_delegated', 'executor_completed')
                       OR (kind = 'executor_observed' AND json_extract(event_json, '$.fact.output.type') IN ('tool_start','tool_result')))
                 UNION ALL
                 SELECT sequence, NULL, NULL FROM session_events
                 WHERE json_extract(event_json, '$.session_id') = ?1
                   AND sequence > ?2 AND sequence <= ?3
                 ) ORDER BY sequence LIMIT ?4",
                        )
                        .bind(&session)
                        .bind(progress)
                        .bind(fence)
                        .bind((max_boundaries + 1) as i64)
                        .fetch_all(&mut *tx)
                        .await?;
                        records
                            .into_iter()
                            .map(|row| {
                                Ok((
                                    row.try_get::<i64, _>(0)?,
                                    row.try_get::<Option<String>, _>(1)?,
                                    row.try_get::<Option<String>, _>(2)?,
                                ))
                            })
                            .collect::<Result<Vec<_>, sqlx::Error>>()?
                    };
                    let ready = boundaries.len() <= max_boundaries;
                    let mut processed = progress;
                    for (sequence, invocation, step) in boundaries.iter().take(max_boundaries) {
                        let Some(invocation) = invocation else {
                            let row = workhub::project(&mut tx, *sequence).await?;
                            persist(&mut tx, &session, row).await?;
                            processed = *sequence;
                            continue;
                        };
                        let facts = evidence::selected(
                            &mut tx,
                            invocation,
                            step.as_deref(),
                            *sequence,
                            true,
                        )
                        .await?;
                        let mut view = InvocationView::new(MAX_TEXT_BYTES)?;
                        for fact in facts {
                            if let Fact::WorkhubDelegated { delegation } = &fact.event.fact {
                                let source = crate::workhub::source_message(
                                    &mut tx,
                                    &fact.event.invocation,
                                    Some(&delegation.source_message_event_id),
                                ).await?;
                                if let Some(row) = maka_presentation::workhub::delegated(&fact, &source)? {
                                    persist(&mut tx, &session, row).await?;
                                }
                                continue;
                            }
                            let resolved = if let Fact::ToolSettled {
                                outcome: ToolOutcome::Succeeded { raw, .. },
                                ..
                            } = &fact.event.fact
                            {
                                // Raw's Json/Mcp/Text/Image wrappers exceed their ToolContent
                                // by 2/1/3/12 bytes. The required full Message envelope is
                                // larger than all four differences, so this lower bound can
                                // reject before hydrating up to 64 MiB. Exact row sizing still
                                // runs in presentation; this is not payload verification.
                                if raw.bytes > MAX_TOOL_ROW_BYTES as u64 {
                                    return Err(ProjectionError::TooLarge.into());
                                }
                                Some(
                                    crate::tool_payloads::resolve_in_transaction(
                                        &mut tx,
                                        &fact.event,
                                    )
                                    .await?,
                                )
                            } else {
                                None
                            };
                            for row in view.push_with_tool_output(&fact, resolved.as_ref())? {
                                persist(&mut tx, &session, row).await?;
                            }
                        }
                        processed = *sequence;
                    }
                    let next = if ready { fence } else { processed };
                    sqlx::query(
                        "INSERT INTO transcript_progress VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET through_sequence = excluded.through_sequence",
                    )
                    .bind(&session)
                    .bind(next)
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    Ok(ready)
                })
            })
            .await
    }
}

async fn persist(tx: &mut SqliteConnection, session: &str, row: Row) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(&row.message)?;
    let sequence =
        i64::try_from(row.sequence).map_err(|_| maka_presentation::ProjectionError::OutOfRange)?;
    let size = i64::try_from(bytes.len()).map_err(|_| StoreError::PrefixTooLarge)?;
    let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
    sqlx::query(
        "INSERT OR IGNORE INTO transcript_rows
         (sequence, session_id, turn_id, message_id, payload, digest, total_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(sequence)
    .bind(session)
    .bind(&row.message.turn_id)
    .bind(&row.message.id)
    .bind(&bytes)
    .bind(&digest)
    .bind(size)
    .execute(&mut *tx)
    .await?;
    // Neither a conflicting identity nor a payload update is an idempotent replay.
    let matches: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM transcript_rows
         WHERE sequence = ?1 AND session_id = ?2 AND turn_id = ?3 AND message_id = ?4
         AND payload = ?5 AND digest = ?6 AND total_bytes = ?7)",
    )
    .bind(sequence)
    .bind(session)
    .bind(&row.message.turn_id)
    .bind(&row.message.id)
    .bind(&bytes)
    .bind(&digest)
    .bind(size)
    .fetch_one(&mut *tx)
    .await?;
    if !matches {
        return Err(StoreError::TranscriptConflict);
    }
    Ok(())
}
