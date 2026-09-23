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

use super::{EventLog, Query, StoreError, invalid};
use maka_runtime::accounting::{
    Cost, ModelSummary, ModelTotals, Pending, ProviderSummary, Summary, Tokens, ToolSummary,
    ToolTotals,
};
use sqlx::{Connection, QueryBuilder, Row, Sqlite, sqlite::SqliteRow};

/// Same canonical fence as activity paging, without activity-only filters.
pub struct Report {
    pub through: u64,
    pub summary: Summary,
}

impl EventLog {
    pub async fn usage_summary(&self, query: Query) -> Result<Report, StoreError> {
        self.validate_root()?;
        query.validate()?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let through = query.fence(&mut tx).await?;
                    let mut models = QueryBuilder::<Sqlite>::new("SELECT ");
                    models.push(MODELS);
                    query.restrict(&mut models, through);
                    let model_row = models.build().fetch_one(&mut *tx).await?;
                    let mut tools = QueryBuilder::<Sqlite>::new("SELECT ");
                    tools.push(TOOLS);
                    query.restrict(&mut tools, through);
                    let tool_row = tools.build().fetch_one(&mut *tx).await?;
                    let summary = Summary {
                        pending: Pending {
                            models: pending(&mut tx, &query, through, "model_usage", "started_at")
                                .await?,
                            tools: pending(
                                &mut tx,
                                &query,
                                through,
                                "tool_usage",
                                "json_extract(result, '$.startedAt')",
                            )
                            .await?,
                        },
                        models: read_models(&model_row)?,
                        tools: read_tools(&tool_row)?,
                        by_provider: groups(
                            &mut tx,
                            &query,
                            through,
                            MODELS,
                            "json_extract(quote_json, '$.providerId')",
                        )
                        .await?
                        .iter()
                        .map(|row| {
                            Ok(ProviderSummary {
                                provider_id: row.try_get("key")?,
                                totals: read_models(row)?,
                            })
                        })
                        .collect::<Result<_, StoreError>>()?,
                        by_model: groups(&mut tx, &query, through, MODELS, "model_id")
                            .await?
                            .iter()
                            .map(|row| {
                                Ok(ModelSummary {
                                    model_id: row.try_get("key")?,
                                    totals: read_models(row)?,
                                })
                            })
                            .collect::<Result<_, StoreError>>()?,
                        by_tool: groups(&mut tx, &query, through, TOOLS, "name")
                            .await?
                            .iter()
                            .map(|row| {
                                Ok(ToolSummary {
                                    name: row.try_get("key")?,
                                    totals: read_tools(row)?,
                                })
                            })
                            .collect::<Result<_, StoreError>>()?,
                    };
                    if serde_json::to_vec(&summary)?.len() > 48 * 1024 {
                        return Err(invalid(
                            "Usage summary exceeds response capacity; narrow the time range",
                        ));
                    }
                    tx.commit().await?;
                    Ok(Report { through, summary })
                })
            })
            .await
    }
}

async fn groups(
    connection: &mut sqlx::SqliteConnection,
    query: &Query,
    through: u64,
    columns: &'static str,
    key: &'static str,
) -> Result<Vec<SqliteRow>, StoreError> {
    let mut sql = QueryBuilder::<Sqlite>::new("SELECT ");
    sql.push(key).push(" AS key, ").push(columns);
    query.restrict(&mut sql, through);
    sql.push(" GROUP BY key ORDER BY calls DESC, key LIMIT 129");
    let rows = sql.build().fetch_all(connection).await?;
    if rows.len() > 128 {
        return Err(invalid(
            "Usage breakdown exceeds 128 groups; narrow the time range",
        ));
    }
    Ok(rows)
}

async fn pending(
    connection: &mut sqlx::SqliteConnection,
    query: &Query,
    through: u64,
    table: &'static str,
    started: &'static str,
) -> Result<u64, StoreError> {
    let mut sql = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) AS calls FROM ");
    sql.push(table)
        .push(" WHERE ")
        .push(started)
        .push(" >= ")
        .push_bind(query.from)
        .push(" AND ")
        .push(started)
        .push(" <= ")
        .push_bind(query.to)
        .push(" AND sequence <= ")
        .push_bind(through as i64)
        .push(" AND (completed_sequence IS NULL OR completed_sequence > ")
        .push_bind(through as i64)
        .push(")");
    if let Some(session) = &query.session_id {
        sql.push(" AND session_id = ").push_bind(session.clone());
    }
    count(&sql.build().fetch_one(connection).await?, "calls")
}

const MODELS: &str = "COUNT(*) AS calls,
    COALESCE(SUM(outcome = 'success'), 0) AS success,
    COALESCE(SUM(outcome = 'error'), 0) AS error,
    COALESCE(SUM(outcome = 'aborted'), 0) AS aborted,
    COALESCE(SUM(outcome = 'unknown'), 0) AS unknown,
    TOTAL(CASE WHEN outcome != 'unknown' THEN MAX(completed_at - started_at, 0.0) END) AS duration,
    TOTAL(json_extract(usage, '$.input_tokens')) AS input,
    COUNT(*) - COUNT(json_extract(usage, '$.input_tokens')) AS input_missing,
    TOTAL(json_extract(usage, '$.output_tokens')) AS output,
    COUNT(*) - COUNT(json_extract(usage, '$.output_tokens')) AS output_missing,
    TOTAL(json_extract(usage, '$.cache_read_tokens')) AS cache_read,
    COUNT(*) - COUNT(json_extract(usage, '$.cache_read_tokens')) AS cache_read_missing,
    TOTAL(json_extract(usage, '$.cache_write_tokens')) AS cache_write,
    COUNT(*) - COUNT(json_extract(usage, '$.cache_write_tokens')) AS cache_write_missing,
    TOTAL(json_extract(usage, '$.reasoning_tokens')) AS reasoning,
    COUNT(*) - COUNT(json_extract(usage, '$.reasoning_tokens')) AS reasoning_missing,
    TOTAL(usd) AS usd,
    COUNT(*) - COUNT(usd) AS unvalued,
    COUNT(*) - COUNT(json_extract(quote_json, '$.pricing')) AS unpriced
    FROM model_usage";

const TOOLS: &str = "COUNT(*) AS calls,
    COALESCE(SUM(json_extract(result, '$.outcome') = 'success'), 0) AS success,
    COALESCE(SUM(json_extract(result, '$.outcome') = 'error'), 0) AS error,
    COALESCE(SUM(json_extract(result, '$.outcome') = 'unknown'), 0) AS unknown,
    COALESCE(SUM(json_extract(result, '$.kind') = 'rejected'), 0) AS rejected,
    TOTAL(CASE WHEN json_extract(result, '$.outcome') IN ('success', 'error')
        THEN MAX(completed_at - json_extract(result, '$.startedAt'), 0.0) END) AS duration,
    AVG(CASE WHEN json_extract(result, '$.outcome') IN ('success', 'error')
        THEN MAX(completed_at - json_extract(result, '$.startedAt'), 0.0) END) AS latency
    FROM tool_usage";

fn read_models(row: &SqliteRow) -> Result<ModelTotals, StoreError> {
    Ok(ModelTotals {
        calls: count(row, "calls")?,
        success: count(row, "success")?,
        error: count(row, "error")?,
        aborted: count(row, "aborted")?,
        unknown: count(row, "unknown")?,
        duration_ms: finite(row.try_get("duration")?)?,
        input: tokens(row, "input", "input_missing")?,
        output: tokens(row, "output", "output_missing")?,
        cache_read: tokens(row, "cache_read", "cache_read_missing")?,
        cache_write: tokens(row, "cache_write", "cache_write_missing")?,
        reasoning: tokens(row, "reasoning", "reasoning_missing")?,
        cost: Cost {
            known_usd: finite(row.try_get("usd")?)?,
            unvalued: count(row, "unvalued")?,
            unpriced: count(row, "unpriced")?,
        },
    })
}
fn read_tools(row: &SqliteRow) -> Result<ToolTotals, StoreError> {
    Ok(ToolTotals {
        calls: count(row, "calls")?,
        success: count(row, "success")?,
        error: count(row, "error")?,
        unknown: count(row, "unknown")?,
        rejected: count(row, "rejected")?,
        duration_ms: finite(row.try_get("duration")?)?,
        mean_latency_ms: row
            .try_get::<Option<f64>, _>("latency")?
            .map(finite)
            .transpose()?,
    })
}
const MAX_NUMBER: u64 = (1 << 53) - 1;

fn count(row: &SqliteRow, column: &str) -> Result<u64, StoreError> {
    let value = crate::sequence_number(row.try_get(column)?)?;
    if value > MAX_NUMBER {
        return Err(invalid("Usage aggregate exceeds exact integer capacity"));
    }
    Ok(value)
}
fn tokens(row: &SqliteRow, value: &str, missing: &str) -> Result<Tokens, StoreError> {
    // SQLite's TOTAL avoids signed-integer overflow on provider u64 counters.
    // Only exact interoperable integer totals are exposed; overflow is not zero.
    let known = finite(row.try_get(value)?)?;
    if known > MAX_NUMBER as f64 || known.fract() != 0.0 {
        return Err(invalid("Usage aggregate exceeds exact integer capacity"));
    }
    Ok(Tokens {
        known: known as u64,
        missing: count(row, missing)?,
    })
}
fn finite(value: f64) -> Result<f64, StoreError> {
    if !value.is_finite() || value < 0.0 {
        return Err(invalid("Usage aggregate exceeds finite numeric capacity"));
    }
    Ok(value)
}
