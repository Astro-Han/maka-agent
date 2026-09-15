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

use super::MAX_SAFE_INTEGER;
use crate::StoreError;
use sqlx::{Connection, SqliteConnection};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "partial.rs"]
mod partial;

pub(crate) fn register_functions(connection: &rusqlite::Connection) -> Result<(), StoreError> {
    use rusqlite::functions::FunctionFlags;
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    connection.create_scalar_function("catalog_time", 1, flags, |ctx| {
        let time: SystemTime = serde_json::from_str(ctx.get_raw(0).as_str()?)
            .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))?;
        time.duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .filter(|value| *value <= MAX_SAFE_INTEGER)
            .map(|value| value as i64)
            .ok_or(rusqlite::Error::InvalidQuery)
    })?;
    partial::register_function(connection)
}

pub(crate) async fn initialize_execution(
    connection: &mut SqliteConnection,
) -> Result<(), StoreError> {
    partial::initialize(connection).await?;
    sqlx::raw_sql(
        "CREATE INDEX IF NOT EXISTS catalog_message_facts ON event_log(
            json_extract(event_json, '$.invocation.session_id'), kind, sequence
         ) WHERE kind IN ('invocation_opened', 'model_completed');
         CREATE INDEX IF NOT EXISTS catalog_part_starts ON event_log(
            json_extract(event_json, '$.invocation.session_id'), invocation_id,
            json_extract(event_json, '$.fact.step_id'), sequence
         ) WHERE kind = 'model_observed'
           AND json_extract(event_json, '$.fact.event.kind') = 'part_started';",
    )
    .execute(&mut *connection)
    .await?;

    // This cache and its watermark are disposable. Rebuild if either is absent;
    // creation, backfill, and its fence commit atomically before serving readers.
    let mut tx = connection.begin().await?;
    let complete: bool = sqlx::query_scalar(
        "SELECT count(*) = 2 FROM sqlite_schema WHERE type = 'table'
         AND name IN ('catalog_messages', 'catalog_message_watermark')",
    )
    .fetch_one(&mut *tx)
    .await?;
    let versioned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('catalog_message_watermark')
            WHERE name = 'projection_version')",
    )
    .fetch_one(&mut *tx)
    .await?;
    let version = if versioned {
        sqlx::query_scalar::<_, i64>(
            "SELECT projection_version FROM catalog_message_watermark WHERE singleton = 1",
        )
        .fetch_optional(&mut *tx)
        .await?
    } else {
        // Only disposable projection metadata changes; canonical events stay intact.
        sqlx::raw_sql("DROP TABLE IF EXISTS catalog_message_watermark;")
            .execute(&mut *tx)
            .await?;
        None
    };
    let complete = complete && version == Some(8);
    if !complete {
        // The schema belongs exclusively to this disposable projection.
        sqlx::raw_sql("DROP TABLE IF EXISTS catalog_messages;")
            .execute(&mut *tx)
            .await?;
    }
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS catalog_messages (
            sequence INTEGER NOT NULL, ordinal INTEGER NOT NULL, session_id TEXT NOT NULL,
            message_at INTEGER NOT NULL, preview TEXT, message_id TEXT NOT NULL,
            PRIMARY KEY (sequence, ordinal)
         );
         CREATE INDEX IF NOT EXISTS catalog_visible_tail ON catalog_messages(
            session_id, sequence DESC, ordinal DESC
         );
         CREATE INDEX IF NOT EXISTS catalog_latest_message ON catalog_messages(
            session_id, message_at DESC, sequence DESC, ordinal DESC
         );
         CREATE INDEX IF NOT EXISTS catalog_latest_preview ON catalog_messages(
            session_id, message_at DESC, sequence DESC, ordinal DESC
         ) WHERE preview IS NOT NULL;
         CREATE TABLE IF NOT EXISTS catalog_message_watermark(
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1), sequence INTEGER NOT NULL,
            projection_version INTEGER NOT NULL
         );
         INSERT OR IGNORE INTO catalog_message_watermark VALUES (1, 0, 8);",
    )
    .execute(&mut *tx)
    .await?;
    if !complete {
        sqlx::raw_sql("UPDATE catalog_message_watermark SET sequence = 0, projection_version = 8;")
            .execute(&mut *tx)
            .await?;
    }
    let mut through: i64 =
        sqlx::query_scalar("SELECT sequence FROM catalog_message_watermark WHERE singleton = 1")
            .fetch_one(&mut *tx)
            .await?;
    loop {
        // Startup only: at most 128 small row identifiers, never event JSON in Rust.
        let rows = sqlx::query_scalar::<_, i64>(
                "SELECT sequence FROM runtime_events WHERE sequence > ?
             AND kind IN ('invocation_opened', 'message_steered', 'model_completed', 'model_interrupted', 'invocation_ended')
             ORDER BY sequence LIMIT 128",
            ).bind(through).fetch_all(&mut *tx).await?;
        if rows.is_empty() {
            break;
        }
        for sequence in rows {
            project_execution(&mut tx, sequence as u64).await?;
            through = sequence;
        }
    }
    sqlx::query(
        "UPDATE catalog_message_watermark SET sequence =
            (SELECT COALESCE(max(sequence), 0) FROM runtime_events)",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Called only for committed catalog boundaries, in the event append transaction.
/// This is a rebuildable index of message facts, not another execution authority.
pub(crate) async fn project_execution(
    connection: &mut SqliteConnection,
    sequence: u64,
) -> Result<(), StoreError> {
    let sequence = sequence as i64;
    let (session, invocation, step, kind): (String, String, Option<String>, String) =
        sqlx::query_as(
            "SELECT json_extract(event_json, '$.invocation.session_id'), invocation_id,
                operation_id, kind FROM runtime_events WHERE sequence = ?",
        )
        .bind(sequence)
        .fetch_one(&mut *connection)
        .await?;
    // Both accepted and interrupted summary text belong to context evidence,
    // not the visible message tail used by previews and read markers.
    let compact: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id = ?
         AND kind = 'invocation_opened'
         AND json_extract(event_json, '$.fact.input.kind') = 'context_compact')",
    )
    .bind(&invocation)
    .fetch_one(&mut *connection)
    .await?;
    let hidden_step = if matches!(kind.as_str(), "model_completed" | "model_interrupted") {
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM runtime_events request
             JOIN runtime_events opening ON opening.invocation_id = request.invocation_id
             AND opening.kind = 'invocation_opened'
             WHERE request.kind = 'model_requested' AND request.invocation_id = ?1
             AND request.operation_id = ?2
             AND json_extract(opening.event_json, '$.fact.input.kind') IN ('message', 'continuation', 'handoff')
             AND COALESCE(json_extract(request.event_json, '$.fact.purpose'), 'main') = 'main')",
        )
        .bind(&invocation)
        .bind(&step)
        .fetch_one(&mut *connection)
        .await?
    } else {
        false
    };
    if compact || hidden_step {
        sqlx::query(
            "UPDATE catalog_message_watermark SET sequence = max(sequence, ?) WHERE singleton = 1",
        )
        .bind(sequence)
        .execute(connection)
        .await?;
        return Ok(());
    }
    let prior_clock: i64 = sqlx::query_scalar(
        "SELECT message_at FROM catalog_messages INDEXED BY catalog_latest_message
         WHERE session_id = ? ORDER BY message_at DESC, sequence DESC, ordinal DESC LIMIT 1",
    )
    .bind(&session)
    .fetch_optional(&mut *connection)
    .await?
    .unwrap_or(0);
    if kind == "invocation_opened" {
        sqlx::query(
            "INSERT OR IGNORE INTO catalog_messages
             SELECT sequence, 0, ?2, catalog_time(json_extract(event_json, '$.recorded_at')),
                catalog_preview(COALESCE(
                    json_extract(event_json, '$.fact.input.content.display_text'),
                    json_extract(event_json, '$.fact.input.content.text'))),
                CASE WHEN json_array_length(event_json, '$.fact.input.source_messages') = 1
                    THEN json_extract(event_json, '$.fact.input.source_messages[0].message_id') ELSE event_id END
             FROM runtime_events WHERE sequence = ?1
             AND json_extract(event_json, '$.fact.input.kind') = 'message'",
        )
        .bind(sequence)
        .bind(&session)
        .execute(&mut *connection)
        .await?;
    } else if kind == "message_steered" {
        crate::steering::project_catalog(connection, sequence, &session).await?;
    } else if kind == "model_completed" {
        // Current step only, excluding every delta and every earlier completion.
        // Accepted text keeps the PartStarted time used by transcript projection.
        sqlx::query(
            "WITH starts AS (
                SELECT catalog_time(json_extract(event_json, '$.recorded_at')) AS ts, event_id,
                    row_number() OVER (ORDER BY sequence) AS ordinal
                FROM event_log INDEXED BY catalog_part_starts
                WHERE json_extract(event_json, '$.invocation.session_id') = ?2
                AND invocation_id = ?3 AND json_extract(event_json, '$.fact.step_id') = ?4
                AND kind = 'model_observed'
                AND json_extract(event_json, '$.fact.event.kind') = 'part_started'
             ), accepted AS (
                SELECT row_number() OVER (ORDER BY CAST(part.key AS INTEGER)) AS ordinal,
                    CASE WHEN json_extract(part.value, '$.text_kind') = 'text'
                        THEN catalog_preview(json_extract(part.value, '$.text')) END AS preview
                FROM runtime_events AS event, json_each(event.event_json, '$.fact.output.parts') AS part
                WHERE event.sequence = ?1 AND json_extract(part.value, '$.kind') = 'text'
             )
             INSERT OR IGNORE INTO catalog_messages
             SELECT ?1, ordinal, ?2, starts.ts, accepted.preview, starts.event_id
             FROM starts JOIN accepted USING (ordinal)",
        ).bind(sequence).bind(&session).bind(&invocation).bind(&step)
            .execute(&mut *connection).await?;
    }
    if kind == "model_interrupted" || kind == "invocation_ended" {
        partial::project(connection, sequence, &session, &invocation, step.as_deref()).await?;
    }
    // TS rejects preview updates older than the previous message clock, even if
    // that clock came from a thinking-only message without any preview. Apply the
    // same guard in commit/part order, scanning only this boundary's new rows.
    sqlx::query(
        "UPDATE catalog_messages AS current SET preview = NULL
         WHERE sequence = ?1 AND message_at < max(?2, COALESCE((
            SELECT max(earlier.message_at) FROM catalog_messages AS earlier
            WHERE earlier.sequence = ?1 AND earlier.ordinal < current.ordinal
         ), 0))",
    )
    .bind(sequence)
    .bind(prior_clock)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "UPDATE catalog_message_watermark SET sequence = max(sequence, ?) WHERE singleton = 1",
    )
    .bind(sequence)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Durable user/assistant tail in presentation order, independent of preview clocks.
pub(crate) async fn latest_visible_message(
    connection: &mut SqliteConnection,
    session: &str,
) -> Result<Option<String>, StoreError> {
    Ok(sqlx::query_scalar(
        "SELECT message_id FROM catalog_messages INDEXED BY catalog_visible_tail
         WHERE session_id = ? ORDER BY sequence DESC, ordinal DESC LIMIT 1",
    )
    .bind(session)
    .fetch_optional(connection)
    .await?)
}
