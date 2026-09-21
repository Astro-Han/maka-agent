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
use maka_presentation::{
    navigation::{RecordedTurnState, TurnContribution, TurnLandmark, TurnStateMessage, truncate},
    watermark,
};
use sqlx::{Connection, Row};
use std::collections::HashMap;

mod queries;

pub struct TurnPage {
    pub contributions: Vec<TurnContribution>,
    pub next_position: Option<u64>,
}

impl EventLog {
    /// Visible history watermark, including running Turns, captured with Session existence.
    pub async fn navigation_fence(&self, session: &str) -> Result<Option<u64>, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session)?;
        let session = session.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM session_control WHERE id = ?)",
                    )
                    .bind(&session)
                    .fetch_one(&mut *tx)
                    .await?;
                    if !exists {
                        return Err(StoreError::SessionNotFound);
                    }
                    let raw: Option<i64> = sqlx::query_scalar(queries::FENCE)
                        .bind(&session)
                        .fetch_one(&mut *tx)
                        .await?;
                    raw.map(|n| Ok(watermark(sequence_number(n)?)?)).transpose()
                })
            })
            .await
    }

    /// A bounded message-index walk. Contributions may split a logical Turn;
    /// the original client merges them by Turn ID and recorded-state sequence.
    pub async fn navigation_turns(
        &self,
        session: &str,
        through: u64,
        position: u64,
        limit: usize,
    ) -> Result<TurnPage, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session)?;
        let through_sql = super::read::sql_number(through)?;
        let position = super::read::sql_number(position)?;
        if !(1..=128).contains(&limit) {
            return Err(invalid("invalid contribution limit"));
        }
        let session = session.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    super::read::prepared(&mut tx, &session, through).await?;
                    let rows = sqlx::query(queries::ROWS)
                        .bind(&session)
                        .bind(through_sql)
                        .bind(position)
                        .fetch_all(&mut *tx)
                        .await?;
                    let mut contributions: Vec<TurnContribution> = Vec::new();
                    let mut indices = HashMap::new();
                    let mut next_position = None;
                    // Fixed envelope keys, punctuation and two safe integers fit in128 bytes.
                    let budget = 192 * 1024 - serde_json::to_vec(&session)?.len() - 128;
                    let mut bytes = 0usize;
                    for (index, row) in rows.into_iter().enumerate() {
                        let sequence = sequence_number(row.try_get(0)?)?;
                        let turn: String = row.try_get(1)?;
                        if index == 256
                            || !indices.contains_key(&turn) && contributions.len() == limit
                        {
                            next_position = Some(sequence);
                            break;
                        }
                        let previous = indices.get(&turn).copied();
                        let (mut contribution, old_bytes) = match previous {
                            Some(index) => {
                                let old: &TurnContribution = &contributions[index];
                                (old.clone(), serde_json::to_vec(old)?.len() + 1)
                            }
                            None => (
                                TurnContribution {
                                    turn_id: turn.clone(),
                                    first_sequence: sequence,
                                    latest_state: None,
                                    user_prompt_preview: None,
                                },
                                0,
                            ),
                        };
                        if contribution.user_prompt_preview.is_none() {
                            contribution.user_prompt_preview = row.try_get(2)?;
                        }
                        if let Some(bytes) = row.try_get::<Option<Vec<u8>>, _>(3)? {
                            let mut message: TurnStateMessage = serde_json::from_slice(&bytes)?;
                            if message.turn_id != contribution.turn_id {
                                return Err(StoreError::TranscriptConflict);
                            }
                            message.abort_source =
                                message.abort_source.map(|s| truncate(&s, 128).to_owned());
                            message.error_class =
                                message.error_class.map(|s| truncate(&s, 128).to_owned());
                            contribution.latest_state =
                                Some(RecordedTurnState { sequence, message });
                        }
                        let updated_bytes =
                            bytes - old_bytes + serde_json::to_vec(&contribution)?.len() + 1;
                        if updated_bytes > budget {
                            if contributions.is_empty() {
                                return Err(StoreError::PrefixTooLarge);
                            }
                            next_position = Some(sequence);
                            break;
                        }
                        bytes = updated_bytes;
                        if let Some(index) = previous {
                            contributions[index] = contribution;
                        } else {
                            indices.insert(turn, contributions.len());
                            contributions.push(contribution);
                        }
                    }
                    Ok(TurnPage {
                        contributions,
                        next_position,
                    })
                })
            })
            .await
    }

    /// Rank unique Turn openings in SQL; load only selected labels and row bounds.
    pub async fn navigation_landmarks(
        &self,
        session: &str,
        through: u64,
        limit: usize,
        turn: Option<&str>,
    ) -> Result<Vec<TurnLandmark>, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session)?;
        let through_sql = super::read::sql_number(through)?;
        if let Some(turn) = turn {
            sessions::validate_id(turn)?;
        }
        let turn = turn.map(str::to_owned);
        if !(1..=64).contains(&limit) {
            return Err(invalid("invalid landmark limit"));
        }
        let session = session.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    super::read::prepared(&mut tx, &session, through).await?;
                    if let Some(turn) = turn {
                        let bounds: (Option<i64>, Option<i64>) = sqlx::query_as(
                            "SELECT MIN(sequence), MAX(sequence) FROM transcript_rows
                             WHERE session_id = ?1 AND turn_id = ?2 AND sequence <= ?3",
                        ).bind(&session).bind(&turn).bind(through_sql).fetch_one(&mut *tx).await?;
                        let Some((first, last)) = bounds.0.zip(bounds.1) else { return Ok(vec![]); };
                        let label: Option<String> = sqlx::query_scalar(
                            "SELECT navigation_preview(COALESCE(json_extract(row.payload, '$.displayText'),
                                json_extract(row.payload, '$.text')), 96)
                             FROM transcript_rows row JOIN runtime_events source ON source.sequence = row.sequence / 256
                             WHERE row.session_id = ?1 AND row.turn_id = ?2 AND row.sequence <= ?3
                             AND source.kind IN ('invocation_opened', 'message_steered') ORDER BY row.sequence LIMIT 1",
                        ).bind(&session).bind(&turn).bind(through_sql).fetch_optional(&mut *tx).await?.flatten();
                        return Ok(vec![TurnLandmark {
                            turn_id: turn, sequence: sequence_number(first)?, last_sequence: sequence_number(last)?,
                            label: label.unwrap_or_default(),
                        }]);
                    }
                    let selected: Vec<String> = sqlx::query_scalar(queries::LANDMARKS)
                        .bind(&session)
                        .bind(through_sql / 256)
                        .bind(limit as i64)
                        .fetch_all(&mut *tx)
                        .await?;
                    let mut result = Vec::new();
                    for invocation in selected {
                        if let Some(row) = sqlx::query(queries::PROMPT)
                            .bind(&session)
                            .bind(&invocation)
                            .bind(through_sql)
                            .fetch_optional(&mut *tx)
                            .await?
                        {
                            let label: Option<String> = row.try_get(2)?;
                            if let Some(label) = label {
                                let turn_id: String = row.try_get(1)?;
                                let (first, last): (i64, i64) = sqlx::query_as(
                                    "SELECT MIN(sequence), MAX(sequence) FROM transcript_rows
                                     WHERE session_id = ?1 AND turn_id = ?2 AND sequence <= ?3",
                                ).bind(&session).bind(&turn_id).bind(through_sql).fetch_one(&mut *tx).await?;
                                result.push(TurnLandmark {
                                    sequence: sequence_number(first)?,
                                    last_sequence: sequence_number(last)?,
                                    turn_id,
                                    label,
                                });
                            }
                        }
                    }
                    Ok(result)
                })
            })
            .await
    }
}

/// SQLite borrows the selected string; only the bounded preview is allocated in Rust.
pub(crate) fn register(connection: &rusqlite::Connection) -> Result<(), StoreError> {
    use rusqlite::functions::FunctionFlags;
    connection.create_scalar_function(
        "navigation_preview",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let text = ctx.get_raw(0).as_str()?;
            let bytes =
                usize::try_from(ctx.get::<i64>(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let text = maka_presentation::navigation::prompt_preview(text, bytes);
            Ok((!text.is_empty()).then(|| text.to_owned()))
        },
    )?;
    Ok(())
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
