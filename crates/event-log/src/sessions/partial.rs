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

use crate::StoreError;
use futures_util::TryStreamExt;
use rusqlite::{Connection, functions::FunctionFlags};
use sqlx::{Row, SqliteConnection};

/// Incremental normalization preserves whitespace across delta boundaries while
/// retaining at most 96 Unicode characters, including a truncation ellipsis.
#[derive(Default)]
struct Preview {
    text: String,
    count: usize,
    space: bool,
    full: bool,
}
impl Preview {
    fn push(&mut self, text: &str) {
        if self.full {
            return;
        }
        for ch in text.chars() {
            if matches!(ch, '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
            {
                self.space = !self.text.is_empty();
                continue;
            }
            for next in self
                .space
                .then_some(' ')
                .into_iter()
                .chain(std::iter::once(ch))
            {
                if self.count == 96 {
                    self.text.pop();
                    self.text.push('…');
                    self.full = true;
                    return;
                }
                self.text.push(next);
                self.count += 1;
            }
            self.space = false;
        }
    }
    fn finish(self) -> Option<String> {
        (!self.text.is_empty()).then_some(self.text)
    }
}
pub(super) fn register_function(connection: &Connection) -> Result<(), StoreError> {
    connection.create_scalar_function(
        "catalog_preview",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let mut preview = Preview::default();
            if let Some(text) = ctx.get_raw(0).as_str_or_null()? {
                preview.push(text);
            }
            Ok(preview.finish())
        },
    )?;
    Ok(())
}

pub(super) async fn initialize(connection: &mut SqliteConnection) -> Result<(), StoreError> {
    sqlx::raw_sql(
        "CREATE INDEX IF NOT EXISTS catalog_partial_deltas ON event_log(
            invocation_id, json_extract(event_json, '$.fact.step_id'),
            json_extract(event_json, '$.fact.event.data.id'), sequence
         ) WHERE kind = 'model_observed'
           AND json_extract(event_json, '$.fact.event.kind') = 'part_delta';
         CREATE INDEX IF NOT EXISTS catalog_model_boundaries ON event_log(
            invocation_id, kind, sequence
         ) WHERE kind IN ('model_requested', 'model_completed', 'model_interrupted');",
    )
    .execute(connection)
    .await?;
    Ok(())
}

/// Only the sealed current step contributes partial messages. Nothing is added
/// to model_completed, so accepted model history retains its existing authority.
pub(super) async fn project(
    connection: &mut SqliteConnection,
    boundary: i64,
    session: &str,
    invocation: &str,
    interrupted_step: Option<&str>,
) -> Result<(), StoreError> {
    let step = if let Some(step) = interrupted_step {
        Some(step.to_owned())
    } else {
        // Failed/cancelled terminal fallback: exclude steps already sealed before
        // this boundary, including during a rebuild with later events present.
        sqlx::query_scalar::<_, String>(
            "WITH request AS (SELECT operation_id FROM event_log INDEXED BY catalog_model_boundaries
             WHERE invocation_id = ?1 AND sequence < ?2
             AND kind IN ('model_requested', 'model_completed', 'model_interrupted')
             AND kind = 'model_requested' ORDER BY sequence DESC LIMIT 1)
             SELECT operation_id FROM request WHERE NOT EXISTS (SELECT 1 FROM event_log AS result INDEXED BY operation_fact
                WHERE result.operation_id = request.operation_id
                AND result.operation_id IS NOT NULL
                AND result.invocation_id = ?1 AND result.sequence < ?2
                AND result.kind IN ('model_completed', 'model_interrupted'))
             AND EXISTS (SELECT 1 FROM runtime_events WHERE sequence = ?2
                AND json_extract(event_json, '$.fact.outcome.kind') IN ('failed', 'cancelled'))
             LIMIT 1",
        ).bind(invocation).bind(boundary).fetch_optional(&mut *connection).await?
    };
    let Some(step) = step else {
        return Ok(());
    };
    let visible: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM runtime_events request
             JOIN runtime_events opening ON opening.invocation_id = request.invocation_id
             AND opening.kind = 'invocation_opened'
             WHERE request.kind = 'model_requested' AND request.invocation_id = ?1
             AND request.operation_id = ?2
             AND json_extract(opening.event_json, '$.fact.input.kind') IN ('message', 'continuation')
             AND COALESCE(json_extract(request.event_json, '$.fact.purpose'), 'main') = 'main')",
    )
    .bind(invocation)
    .bind(&step)
    .fetch_one(&mut *connection)
    .await?;
    if !visible {
        return Ok(());
    }
    let starts = sqlx::query(
        "SELECT sequence, catalog_time(json_extract(event_json, '$.recorded_at')),
            json_extract(event_json, '$.fact.event.data.id'),
            json_extract(event_json, '$.fact.event.data.text_kind') = 'text', event_id
         FROM event_log INDEXED BY catalog_part_starts
         WHERE json_extract(event_json, '$.invocation.session_id') = ?1
         AND invocation_id = ?2 AND json_extract(event_json, '$.fact.step_id') = ?3
         AND kind = 'model_observed'
         AND json_extract(event_json, '$.fact.event.kind') = 'part_started'
         AND sequence < ?4 ORDER BY sequence LIMIT 129",
    )
    .bind(session)
    .bind(invocation)
    .bind(&step)
    .bind(boundary)
    .fetch_all(&mut *connection)
    .await?;
    if starts.len() > 128 {
        return Err(StoreError::InvalidTransition(
            "catalog partial exceeds 128 text parts".into(),
        ));
    }
    for (ordinal, row) in starts.into_iter().enumerate() {
        let start: i64 = row.try_get(0)?;
        let timestamp: i64 = row.try_get(1)?;
        let part: String = row.try_get(2)?;
        let is_text: bool = row.try_get(3)?;
        let message_id: String = row.try_get(4)?;
        let mut preview = Preview::default();
        if is_text {
            let mut rows = sqlx::query(
                "SELECT json_extract(event_json, '$.fact.event.data.text')
                 FROM event_log INDEXED BY catalog_partial_deltas
                 WHERE invocation_id = ?1 AND json_extract(event_json, '$.fact.step_id') = ?2
                 AND json_extract(event_json, '$.fact.event.data.id') = ?3
                 AND kind = 'model_observed'
                 AND json_extract(event_json, '$.fact.event.kind') = 'part_delta'
                 AND sequence > ?4 AND sequence < ?5 ORDER BY sequence",
            )
            .bind(invocation)
            .bind(&step)
            .bind(part)
            .bind(start)
            .bind(boundary)
            .fetch(&mut *connection);
            while !preview.full {
                let Some(row) = rows.try_next().await? else {
                    break;
                };
                // Borrow each SQLite value; never concatenate or deserialize the
                // complete stream. Once the prefix is full, stop reading deltas.
                preview.push(row.try_get::<&str, _>(0)?);
            }
        }
        sqlx::query("INSERT OR IGNORE INTO catalog_messages VALUES (?, ?, ?, ?, ?, ?)")
            .bind(boundary)
            .bind(ordinal as i64 + 1)
            .bind(session)
            .bind(timestamp)
            .bind(preview.finish())
            .bind(message_id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}
