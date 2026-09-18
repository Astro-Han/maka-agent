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

use crate::{EventLog, StoreError, sequence_number, sessions};
use maka_presentation::watermark;
use sqlx::{Row, SqliteConnection};

#[derive(Clone, Copy, Debug)]
pub enum TranscriptDirection {
    Older,
    Newer,
}

pub struct TranscriptRead {
    pub through: u64,
    /// Inclusive row position. Public anchors are made exclusive by the pager.
    pub position: u64,
    pub direction: TranscriptDirection,
    pub limit: usize,
}
#[derive(Debug)]
pub struct TranscriptRowHeader {
    pub sequence: u64,
    pub turn_id: String,
    pub total_bytes: u64,
    pub digest: String,
}
#[derive(Debug)]
pub struct TranscriptTurnBounds {
    pub first: u64,
    pub last: u64,
    pub rows: u64,
    pub bytes: u64,
}

impl EventLog {
    /// True only when no logical Turn straddles the cut, including resumed Turns.
    pub async fn transcript_between_turns(
        &self,
        session: &str,
        through: u64,
        cut: u64,
    ) -> Result<bool, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session)?;
        let session = session.to_owned();
        let through_sql = sql_number(through)?;
        let cut = sql_number(cut)?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    prepared(connection, &session, through).await?;
                    let crossing: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM transcript_rows
                 WHERE session_id = ?1 AND sequence <= ?2
                 GROUP BY turn_id HAVING MIN(sequence) <= ?3 AND MAX(sequence) > ?3)",
                    )
                    .bind(session)
                    .bind(through_sql)
                    .bind(cut)
                    .fetch_one(connection)
                    .await?;
                    Ok(!crossing)
                })
            })
            .await
    }

    pub async fn transcript_headers(
        &self,
        session: &str,
        read: &TranscriptRead,
    ) -> Result<Vec<TranscriptRowHeader>, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session)?;
        if read.limit == 0 || read.limit > 256 {
            return Err(invalid("invalid transcript header limit"));
        }
        let through = sql_number(read.through)?;
        let position = sql_number(read.position)?;
        let session = session.to_owned();
        let watermark = read.through;
        let direction = read.direction;
        let limit = read.limit as i64;
        self.connection.run(move |connection| Box::pin(async move {
        prepared(connection, &session, watermark).await?;
        let query = match direction {
            TranscriptDirection::Older =>
                "SELECT sequence, turn_id, total_bytes, digest FROM transcript_rows
                 WHERE session_id = ?1 AND sequence <= ?2 AND sequence <= ?3 ORDER BY sequence DESC LIMIT ?4",
            TranscriptDirection::Newer =>
                "SELECT sequence, turn_id, total_bytes, digest FROM transcript_rows
                 WHERE session_id = ?1 AND sequence <= ?2 AND sequence >= ?3 ORDER BY sequence ASC LIMIT ?4",
        };
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(&session).bind(through).bind(position).bind(limit).fetch_all(connection).await?;
        let mut headers = Vec::new();
        for row in rows {
            headers.push(TranscriptRowHeader {
                sequence: sequence_number(row.try_get(0)?)?,
                turn_id: row.try_get(1)?,
                total_bytes: sequence_number(row.try_get(2)?)?,
                digest: row.try_get(3)?,
            });
        }
        Ok(headers)
        })).await
    }

    /// Slices immutable UTF-8 JSON bytes as a BLOB; a fragment can end inside a
    /// code point because the client decodes only after whole-message assembly.
    pub async fn transcript_fragment(
        &self,
        session: &str,
        sequence: u64,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session)?;
        if length == 0 || length > 512 * 1024 {
            return Err(invalid("invalid transcript fragment length"));
        }
        let sequence = sql_number(sequence)?;
        let start = offset
            .checked_add(1)
            .ok_or_else(|| invalid("invalid transcript byte offset"))?;
        let start = sql_number(start)?;
        let session = session.to_owned();
        self.connection.run(move |connection| Box::pin(async move {
        let total: i64 = sqlx::query_scalar(
                "SELECT total_bytes FROM transcript_rows WHERE session_id = ?1 AND sequence = ?2",
            ).bind(&session).bind(sequence).fetch_optional(&mut *connection).await?
            .ok_or_else(|| invalid("transcript message was not found"))?;
        let total = sequence_number(total)?;
        if offset.checked_add(length).is_none_or(|end| end > total) {
            return Err(invalid("transcript fragment exceeds message"));
        }
        let bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT substr(payload, ?3, ?4) FROM transcript_rows WHERE session_id = ?1 AND sequence = ?2",
        ).bind(&session).bind(sequence).bind(start).bind(length as i64)
            .fetch_one(connection).await?;
        if bytes.len() as u64 != length {
            return Err(invalid("transcript payload length changed"));
        }
        Ok(bytes)
        })).await
    }

    pub async fn transcript_turn_bounds(
        &self,
        session: &str,
        turn: &str,
        through: u64,
    ) -> Result<Option<TranscriptTurnBounds>, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session)?;
        sessions::validate_id(turn)?;
        let through_sql = sql_number(through)?;
        let session = session.to_owned();
        let turn = turn.to_owned();
        self.connection.run(move |connection| Box::pin(async move {
        prepared(connection, &session, through).await?;
        let (first, last, rows, bytes): (Option<i64>, Option<i64>, i64, i64) = sqlx::query_as(
                "SELECT MIN(sequence), MAX(sequence), COUNT(*), COALESCE(SUM(total_bytes), 0)
             FROM transcript_rows WHERE session_id = ?1 AND turn_id = ?2 AND sequence <= ?3",
            ).bind(&session).bind(&turn).bind(through_sql).fetch_one(connection).await?;
        first
            .zip(last)
            .map(|(first, last)| {
                Ok(TranscriptTurnBounds {
                    first: sequence_number(first)?,
                    last: sequence_number(last)?,
                    rows: sequence_number(rows)?,
                    bytes: sequence_number(bytes)?,
                })
            })
            .transpose()
        })).await
    }
}

pub(super) async fn prepared(
    connection: &mut SqliteConnection,
    session: &str,
    through: u64,
) -> Result<(), StoreError> {
    let source: Option<i64> =
        sqlx::query_scalar("SELECT through_sequence FROM transcript_progress WHERE session_id = ?")
            .bind(session)
            .fetch_optional(connection)
            .await?;
    let Some(source) = source else {
        return Err(invalid("transcript index is not prepared"));
    };
    if through > watermark(sequence_number(source)?)? {
        return Err(invalid("transcript watermark is not prepared"));
    }
    Ok(())
}
pub(super) fn sql_number(value: u64) -> Result<i64, StoreError> {
    if value > 9_007_199_254_740_991 {
        return Err(invalid("invalid transcript integer"));
    }
    Ok(value as i64)
}
fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
