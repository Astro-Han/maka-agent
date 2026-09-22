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

use super::read::{prepared, sql_number};
use crate::{EventLog, StoreError, sequence_number};
use maka_plugins::session::history::{Chunk, Cursor, Page, Role};
use sqlx::{Connection, Row};

const CHUNK_BYTES: u64 = 16 * 1024;
const PAGE_CHUNKS: usize = 8;

impl EventLog {
    /// Exact text fragments from the disposable transcript index. BLOB slicing
    /// preserves embedded NULs; only a complete UTF-8 prefix leaves this boundary.
    /// A long message continues before the next one, so no suffix is skipped.
    pub async fn history_text(
        &self,
        session: &str,
        through: u64,
        cursor: Option<Cursor>,
    ) -> Result<Page, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session)?;
        let through_sql = sql_number(maka_presentation::watermark(through)?)?;
        let cursor = cursor.unwrap_or_default();
        let sequence = sql_number(cursor.sequence)?;
        if sequence > through_sql {
            return Err(invalid("history cursor exceeds its fence"));
        }
        let offset = sql_number(cursor.offset)?;
        let session = session.to_owned();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin().await?;
            prepared(&mut tx, &session, through).await?;
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM session_control WHERE id = ?)"
            ).bind(&session).fetch_one(&mut *tx).await?;
            if !exists { return Err(invalid("Session no longer exists")); }
            // Build each immutable text projection once, independently of query
            // terms. Bound backfill by source rows and bytes; an oversized first
            // row still progresses. A partial message never prefetches later rows.
            sqlx::query(
                "WITH source AS MATERIALIZED (
                    SELECT sequence, total_bytes FROM transcript_rows
                    WHERE session_id = ?1 AND sequence >= ?2 AND sequence <= ?3
                    ORDER BY sequence LIMIT ?4
                ), missing AS (
                    SELECT sequence,
                        COALESCE(SUM(total_bytes) OVER (ORDER BY sequence ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS preceding_bytes
                    FROM source WHERE NOT EXISTS (
                        SELECT 1 FROM transcript_text WHERE transcript_text.sequence = source.sequence)
                )
                INSERT INTO transcript_text
                    SELECT transcript_rows.sequence,
                        json_extract(payload, '$.ts') AS timestamp,
                        json_extract(payload, '$.type') AS role,
                        CAST(COALESCE(CASE json_extract(payload, '$.type')
                            WHEN 'user' THEN COALESCE(json_extract(payload, '$.displayText'), json_extract(payload, '$.text'), '')
                                || COALESCE((SELECT char(10) || group_concat(json_extract(value, '$.name'), char(10))
                                    FROM json_each(payload, '$.attachments')), '')
                            WHEN 'assistant' THEN json_extract(payload, '$.text')
                            WHEN 'tool_call' THEN COALESCE(json_extract(payload, '$.intent'), json_extract(payload, '$.args.intent'), '')
                            WHEN 'tool_result' THEN CASE json_extract(payload, '$.content.kind')
                                WHEN 'text' THEN json_extract(payload, '$.content.text')
                                WHEN 'json' THEN (SELECT group_concat(atom, char(10))
                                    FROM json_tree(payload, '$.content.value') WHERE type = 'text')
                                ELSE '' END
                            ELSE '' END, '') AS BLOB) AS body
                    FROM missing JOIN transcript_rows USING (sequence)
                    WHERE preceding_bytes < 1048576"
            ).bind(&session).bind(sequence).bind(through_sql)
                .bind(if offset > 0 { 1 } else { (PAGE_CHUNKS + 1) as i64 })
                .execute(&mut *tx).await?;
            let rows = sqlx::query(
                "SELECT source.sequence, source.message_id, source.turn_id,
                    cached.sequence IS NOT NULL AS cached, timestamp, role,
                    COALESCE(length(body), 0) AS total,
                    substr(body, CASE WHEN sequence = ?2 THEN ?5 + 1 ELSE 1 END, ?6) AS fragment,
                    CASE WHEN role = 'user' AND (sequence != ?2 OR ?5 = 0)
                        THEN json_extract(source.payload, '$.attachments') END AS attachments
                FROM transcript_rows source LEFT JOIN transcript_text cached USING (sequence)
                WHERE source.session_id = ?1 AND source.sequence >= ?2 AND source.sequence <= ?3
                ORDER BY source.sequence LIMIT ?4"
            ).bind(&session).bind(sequence).bind(through_sql)
                .bind((PAGE_CHUNKS + 1) as i64).bind(offset).bind(CHUNK_BYTES as i64)
                .fetch_all(&mut *tx).await?;
            let mut chunks = Vec::new();
            let mut next = None;
            for (index, row) in rows.iter().enumerate() {
                let sequence = sequence_number(row.try_get("sequence")?)?;
                if index == PAGE_CHUNKS || !row.try_get::<bool, _>("cached")? {
                    next = Some(Cursor { sequence, offset: 0 });
                    break;
                }
                let total_bytes = sequence_number(row.try_get("total")?)?;
                let offset = if sequence == cursor.sequence { cursor.offset } else { 0 };
                if offset > total_bytes { return Err(invalid("history offset exceeds message")); }
                let role = match row.try_get::<String, _>("role")?.as_str() {
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    "tool_call" => Role::ToolCall,
                    "tool_result" => Role::ToolResult,
                    _ => continue,
                };
                if total_bytes == 0 { continue; }
                let bytes = row.try_get::<Option<Vec<u8>>, _>("fragment")?.unwrap_or_default();
                let text = match std::str::from_utf8(&bytes) {
                    Ok(text) => text,
                    Err(error) if error.error_len().is_none() && error.valid_up_to() > 0 => {
                        std::str::from_utf8(&bytes[..error.valid_up_to()])
                            .map_err(|_| invalid("invalid history UTF-8"))?
                    }
                    Err(_) => return Err(invalid("history offset splits UTF-8")),
                }.to_owned();
                let end = offset + text.len() as u64;
                chunks.push(Chunk {
                    message_id: row.try_get("message_id")?,
                    turn_id: row.try_get("turn_id")?,
                    timestamp: sequence_number(row.try_get("timestamp")?)?,
                    role, sequence, offset, total_bytes, text,
                    attachments: row.try_get::<Option<String>, _>("attachments")?
                        .map(|value| serde_json::from_str(&value)).transpose()?.unwrap_or_default(),
                });
                if end < total_bytes {
                    next = Some(Cursor { sequence, offset: end });
                    break;
                }
            }
            tx.commit().await?;
            Ok(Page::Ready { through, chunks, next })
        })).await
    }
}
fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
