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

use futures_util::TryStreamExt;
use maka_runtime::event::Invocation;
use serde_json::Value;
use sqlx::{Connection, Row};

use crate::{EventLog, StoreError};

macro_rules! unresolved {
    ($tail:literal) => { concat!(
        "WITH unresolved(kind, id, name, input, call) AS (
                    SELECT 0, request.operation_id, '', 'null', 'null'
                    FROM runtime_events AS request
                    WHERE request.invocation_id = ?1 AND request.kind = 'model_requested'
                    AND NOT EXISTS(SELECT 1 FROM runtime_events AS result
                        WHERE result.invocation_id = request.invocation_id
                        AND result.operation_id = request.operation_id
                        AND result.kind IN ('model_completed', 'model_interrupted'))
                    UNION ALL
                    SELECT 1, dispatch.operation_id, '', 'null', 'null'
                    FROM runtime_events AS dispatch
                    WHERE dispatch.invocation_id = ?1 AND dispatch.kind = 'tool_dispatched'
                    AND NOT EXISTS(SELECT 1 FROM runtime_events AS result
                        WHERE result.invocation_id = dispatch.invocation_id
                        AND result.operation_id = dispatch.operation_id AND result.kind = 'tool_settled')
                    UNION ALL
                    SELECT 2, completed.operation_id || ':' || json_extract(part.value, '$.call.id'),
                        json_extract(part.value, '$.call.name'), part.value -> '$.call.input',
                        json_object('tool_call_id', json_extract(part.value, '$.call.id'),
                            'origin', json_object('kind', 'provider', 'step_id', completed.operation_id))
                    FROM runtime_events AS completed, json_each(completed.event_json, '$.fact.output.parts') AS part
                    WHERE completed.invocation_id = ?1 AND completed.kind = 'model_completed'
                    AND json_extract(part.value, '$.kind') = 'tool_call'
                    AND json_extract(part.value, '$.call.provider_executed') = 0
                    AND NOT EXISTS(SELECT 1 FROM runtime_events AS dispatch
                        WHERE dispatch.invocation_id = completed.invocation_id AND dispatch.kind IN ('tool_dispatched', 'tool_rejected')
                        AND dispatch.operation_id = completed.operation_id || ':' || json_extract(part.value, '$.call.id'))
                    UNION ALL
                    SELECT 3, started.event_id, '', 'null', 'null'
                    FROM runtime_events started
                    WHERE started.invocation_id = ?1 AND started.kind = 'executor_started'
                    AND NOT EXISTS(SELECT 1 FROM runtime_events done
                        WHERE done.invocation_id = started.invocation_id
                        AND (done.kind = 'executor_completed' OR
                            (done.kind = 'invocation_ended' AND
                             COALESCE(json_extract(done.event_json, '$.fact.outcome.class'), '') != 'outcome_unknown')))
                )",
        $tail
    ) };
}
pub(crate) use unresolved;

pub struct InvocationRecovery {
    pub unfinished_executor: bool,
    pub unfinished_model_steps: Vec<String>,
    pub uncertain_operations: Vec<String>,
    pub undispatched_calls: Vec<UndispatchedCall>,
}

pub struct UndispatchedCall {
    pub operation_id: String,
    pub identity: maka_runtime::tool_call::ToolCallIdentity,
    pub name: String,
    pub input: Value,
}

impl EventLog {
    /// Read only unresolved execution authority in one SQLite snapshot. Text,
    /// streamed observations, opening inputs and settled results are excluded.
    /// Bounds cover the projected operations, not the model-history prefix.
    pub async fn invocation_recovery(
        &self,
        invocation: &Invocation,
        max_operations: usize,
        max_bytes: usize,
    ) -> Result<InvocationRecovery, StoreError> {
        self.validate_root()?;
        let limit = max_operations
            .checked_add(1)
            .and_then(|n| i64::try_from(n).ok())
            .ok_or(StoreError::PrefixTooLarge)?;
        let invocation = invocation.clone();
        self.connection.run(move |connection| Box::pin(async move {
        let mut transaction = connection.begin().await?;
        let identity: String = sqlx::query_scalar(
            "SELECT json_extract(event_json, '$.invocation') FROM runtime_events
             WHERE invocation_id = ? AND kind = 'invocation_opened'",
        ).bind(&invocation.invocation_id).fetch_one(&mut *transaction).await?;
        if serde_json::from_str::<Invocation>(&identity)? != invocation {
            return Err(StoreError::InvalidTransition(
                "stored invocation identity changed".into(),
            ));
        }
        crate::tool_payloads::verify_invocation_bindings(&mut transaction, &invocation.invocation_id).await?;
        let mut evidence = InvocationRecovery {
            unfinished_executor: false,
            unfinished_model_steps: Vec::new(),
            uncertain_operations: Vec::new(),
            undispatched_calls: Vec::new(),
        };
        {
            let query = unresolved!(" SELECT kind, id, name, input, call,
                    length(CAST(id AS BLOB)) + length(CAST(name AS BLOB)) + length(CAST(input AS BLOB)) + length(CAST(call AS BLOB))
                FROM unresolved LIMIT ?2");
            let mut rows = sqlx::query(query).bind(&invocation.invocation_id).bind(limit).fetch(&mut *transaction);
            let mut bytes = 0usize;
            let mut count = 0usize;
            while let Some(row) = rows.try_next().await? {
                let size = usize::try_from(row.try_get::<i64, _>(5)?)
                    .map_err(|_| StoreError::PrefixTooLarge)?;
                bytes = bytes.checked_add(size).ok_or(StoreError::PrefixTooLarge)?;
                if count == max_operations || bytes > max_bytes {
                    return Err(StoreError::PrefixTooLarge);
                }
                count += 1;
                match row.try_get::<i64, _>(0)? {
                    0 => evidence.unfinished_model_steps.push(row.try_get(1)?),
                    1 => evidence.uncertain_operations.push(row.try_get(1)?),
                    2 => evidence.undispatched_calls.push(UndispatchedCall {
                        operation_id: row.try_get(1)?,
                        identity: serde_json::from_str(row.try_get(4)?)?,
                        name: row.try_get(2)?,
                        input: serde_json::from_str(row.try_get(3)?)?,
                    }),
                    3 => evidence.unfinished_executor = true,
                    _ => unreachable!("query emits only recovery fact kinds"),
                }
            }
        }
        transaction.commit().await?;
        Ok(evidence)
        })).await
    }

    /// Enumerate canonical unsealed openings without loading root history or
    /// opening input bodies. Exceeding the bound fails instead of truncating.
    pub async fn unfinished_invocations(
        &self,
        limit: usize,
    ) -> Result<Vec<Invocation>, StoreError> {
        self.validate_root()?;
        let sql_limit = limit
            .checked_add(1)
            .and_then(|n| i64::try_from(n).ok())
            .ok_or(StoreError::PrefixTooLarge)?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut rows = sqlx::query(
                        "SELECT json_extract(opening.event_json, '$.invocation.session_id'),
                    json_extract(opening.event_json, '$.invocation.turn_id'),
                    json_extract(opening.event_json, '$.invocation.run_id'),
                    opening.invocation_id
             FROM runtime_events AS opening
             WHERE opening.kind = 'invocation_opened'
             AND NOT EXISTS(SELECT 1 FROM runtime_events AS terminal
                 WHERE terminal.invocation_id = opening.invocation_id
                 AND terminal.kind = 'invocation_ended')
             ORDER BY opening.sequence LIMIT ?",
                    )
                    .bind(sql_limit)
                    .fetch(connection);
                    let mut openings = Vec::new();
                    while let Some(row) = rows.try_next().await? {
                        if openings.len() == limit {
                            return Err(StoreError::PrefixTooLarge);
                        }
                        openings.push(Invocation {
                            session_id: row.try_get(0)?,
                            turn_id: row.try_get(1)?,
                            run_id: row.try_get(2)?,
                            invocation_id: row.try_get(3)?,
                        });
                    }
                    Ok(openings)
                })
            })
            .await
    }
}
