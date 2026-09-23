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

//! Accounting reads use physical source facts, never copied conversation history.
//! Missing counters remain unknown; this layer makes no pricing assumptions.

use crate::{EventLog, StoreError};
pub use maka_runtime::accounting::{AuxiliarySource, ModelAttempt, Origin, Outcome};
use sqlx::{Connection, Row, sqlite::SqliteRow};

mod auxiliary;
pub(crate) mod valuation;

#[derive(Clone, Debug)]
pub struct Query {
    /// Inclusive Unix milliseconds; fractional milliseconds are not rounded.
    pub from: f64,
    pub to: f64,
    pub session_id: Option<String>,
    /// Inclusive canonical fence from the first page, independent of wall-clock time.
    pub through: Option<u64>,
}

impl Query {
    fn validate(&self) -> Result<(), StoreError> {
        if !self.from.is_finite() || !self.to.is_finite() || self.from < 0.0 || self.to < self.from
        {
            return Err(invalid("invalid Usage time range"));
        }
        if let Some(id) = &self.session_id {
            crate::sessions::validate_id(id)?;
        }
        Ok(())
    }
}

pub struct ModelPage {
    pub through: u64,
    pub attempts: Vec<ModelAttempt>,
    pub total: u64,
    pub next_offset: Option<u64>,
}

impl EventLog {
    /// Bounded, settled model admissions (Agent inference, compaction and Host SDK).
    /// Includes failed physical retries and provider usage from rejected output.
    /// Session removal retains these canonical facts; copies do not duplicate them.
    pub async fn model_attempts(
        &self,
        query: Query,
        offset: u64,
        limit: u32,
    ) -> Result<ModelPage, StoreError> {
        self.validate_root()?;
        query.validate()?;
        if offset > i64::MAX as u64 || !(1..=100).contains(&limit) {
            return Err(invalid("invalid Usage page"));
        }
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let current = crate::sequence_number(
                        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM event_log")
                            .fetch_one(&mut *tx)
                            .await?,
                    )?;
                    let through = query.through.unwrap_or(current);
                    if through > current {
                        return Err(invalid("Usage fence is in the future"));
                    }
                    let total: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM model_usage
                 WHERE completed_at >= ?1 AND completed_at <= ?2
                 AND (?3 IS NULL OR session_id = ?3) AND completed_sequence <= ?4",
                    )
                    .bind(query.from)
                    .bind(query.to)
                    .bind(&query.session_id)
                    .bind(through as i64)
                    .fetch_one(&mut *tx)
                    .await?;
                    let rows = sqlx::query(
                        "SELECT event_id, origin, session_id, binding, model_id,
                        started_at, completed_at, outcome, usage, quote_json, usd
                 FROM model_usage
                 WHERE completed_at >= ?1 AND completed_at <= ?2
                 AND (?3 IS NULL OR session_id = ?3)
                 AND completed_sequence <= ?4
                 ORDER BY completed_at DESC, sequence DESC LIMIT ?5 OFFSET ?6",
                    )
                    .bind(query.from)
                    .bind(query.to)
                    .bind(query.session_id)
                    .bind(through as i64)
                    .bind(limit)
                    .bind(offset as i64)
                    .fetch_all(&mut *tx)
                    .await?;
                    let mut attempts = Vec::with_capacity(rows.len());
                    // Leave envelope/cursor headroom. Never truncate one accounting fact.
                    let mut bytes = 0;
                    for row in &rows {
                        let attempt = read(row)?;
                        let size = serde_json::to_vec(&attempt)?.len() + 1;
                        if bytes + size > 44 * 1024 {
                            if attempts.is_empty() {
                                return Err(invalid("one Usage record exceeds page capacity"));
                            }
                            break;
                        }
                        bytes += size;
                        attempts.push(attempt);
                    }
                    let total = crate::sequence_number(total)?;
                    let end = offset + attempts.len() as u64;
                    tx.commit().await?;
                    Ok(ModelPage {
                        through,
                        attempts,
                        total,
                        next_offset: (end < total).then_some(end),
                    })
                })
            })
            .await
    }
}

fn read(row: &SqliteRow) -> Result<ModelAttempt, StoreError> {
    let outcome: String = row.try_get("outcome")?;
    Ok(ModelAttempt {
        request_id: row.try_get("event_id")?,
        origin: serde_json::from_str(row.try_get("origin")?)?,
        session_id: row.try_get("session_id")?,
        binding: row
            .try_get::<Option<&str>, _>("binding")?
            .map(serde_json::from_str)
            .transpose()?,
        model_id: row.try_get("model_id")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        outcome: serde_json::from_value(serde_json::Value::String(outcome))?,
        usage: serde_json::from_str(row.try_get("usage")?)?,
        quote: row
            .try_get::<Option<&str>, _>("quote_json")?
            .map(serde_json::from_str)
            .transpose()?,
        cost_usd: row.try_get("usd")?,
    })
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
