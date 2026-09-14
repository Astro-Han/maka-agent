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

use super::invalid;
use crate::StoreError;
use maka_runtime::event::RuntimeEvent;
use sqlx::SqliteConnection;

pub(crate) async fn current_opening(
    connection: &mut SqliteConnection,
    session: &str,
    current: Option<&str>,
) -> Result<Option<(i64, RuntimeEvent)>, StoreError> {
    let Some(id) = current else {
        return Ok(None);
    };
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT sequence, event_json FROM runtime_events e WHERE invocation_id = ?
         AND kind = 'invocation_opened' AND json_extract(event_json, '$.invocation.session_id') = ?
         AND NOT EXISTS(SELECT 1 FROM runtime_events t WHERE t.invocation_id = e.invocation_id AND t.kind = 'invocation_ended')",
    ).bind(id).bind(session).fetch_optional(connection).await?;
    let (sequence, json) =
        row.ok_or_else(|| invalid("current invocation is not live in this Session"))?;
    Ok(Some((sequence, serde_json::from_str(&json)?)))
}

/// Checks all old effects, including hidden children and failed-but-unknown runs.
pub(crate) async fn require_safe(
    connection: &mut SqliteConnection,
    session: &str,
    current: Option<&str>,
) -> Result<(), StoreError> {
    require_safe_through(connection, session, current, i64::MAX as u64).await
}

/// Historical observations cannot borrow a later settlement or inherit later work.
/// NOT MATERIALIZED keeps the cut in the indexed queries rather than copying the ledger.
pub(super) async fn require_safe_through(
    connection: &mut SqliteConnection,
    session: &str,
    current: Option<&str>,
    through: u64,
) -> Result<(), StoreError> {
    let unsafe_history: bool = sqlx::query_scalar(
        "WITH runtime_events AS NOT MATERIALIZED (SELECT * FROM main.runtime_events WHERE sequence <= ?3)
         SELECT EXISTS(SELECT 1 FROM runtime_events e
         WHERE json_extract(e.event_json, '$.invocation.session_id') = ?1
         AND (?2 IS NULL OR e.invocation_id != ?2) AND (
           (e.kind = 'invocation_opened' AND NOT EXISTS(SELECT 1 FROM runtime_events t
              WHERE t.invocation_id = e.invocation_id AND t.kind = 'invocation_ended'))
           OR (e.kind = 'tool_dispatched' AND NOT EXISTS(SELECT 1 FROM runtime_events t
              WHERE t.invocation_id = e.invocation_id AND t.operation_id = e.operation_id AND t.kind = 'tool_settled'))
           OR (e.kind = 'model_requested' AND NOT EXISTS(SELECT 1 FROM runtime_events t
              WHERE t.invocation_id = e.invocation_id AND t.operation_id = e.operation_id AND t.kind IN ('model_completed','model_interrupted')))
           OR (e.kind = 'model_completed' AND EXISTS(SELECT 1 FROM json_each(e.event_json, '$.fact.output.parts') p
              WHERE json_extract(p.value, '$.kind') = 'tool_call' AND json_extract(p.value, '$.call.provider_executed') = 0
              AND NOT EXISTS(SELECT 1 FROM runtime_events d WHERE d.invocation_id = e.invocation_id
                AND d.operation_id = e.operation_id || ':' || json_extract(p.value, '$.call.id')
                AND d.kind IN ('tool_dispatched','tool_rejected'))))
           OR (e.kind = 'model_completed' AND EXISTS(SELECT 1 FROM json_each(e.event_json, '$.fact.output.parts') p
              WHERE json_extract(p.value, '$.kind') = 'tool_call' AND json_extract(p.value, '$.call.provider_executed') = 1
              AND NOT EXISTS(SELECT 1 FROM json_each(e.event_json, '$.fact.output.parts') result
                WHERE json_extract(result.value, '$.kind') = 'tool_result'
                  AND json_extract(result.value, '$.id') = json_extract(p.value, '$.call.id')
                  AND json_extract(result.value, '$.name') = json_extract(p.value, '$.call.name'))))))",
    ).bind(session).bind(current).bind(through as i64).fetch_one(&mut *connection).await?;
    if unsafe_history {
        return Err(invalid(
            "Session contains unsealed or unresolved prior execution",
        ));
    }
    // An interrupted model step closes the request, not an observed remote
    // effect. Only that step's durably observed matching result settles it.
    let unknown_provider_effect: bool = sqlx::query_scalar(
        "WITH runtime_events AS NOT MATERIALIZED (SELECT * FROM main.runtime_events WHERE sequence <= ?3)
         SELECT EXISTS(SELECT 1 FROM runtime_events interrupted JOIN runtime_events call
           ON call.invocation_id = interrupted.invocation_id AND call.kind = 'model_observed'
           AND json_extract(call.event_json, '$.fact.step_id') = json_extract(interrupted.event_json, '$.fact.step_id')
           AND call.sequence < interrupted.sequence
         WHERE interrupted.kind = 'model_interrupted'
           AND json_extract(interrupted.event_json, '$.invocation.session_id') = ?1
           AND (?2 IS NULL OR interrupted.invocation_id != ?2)
           AND json_extract(call.event_json, '$.fact.event.kind') = 'tool_call'
           AND json_extract(call.event_json, '$.fact.event.data.provider_executed') = 1
           AND NOT EXISTS(SELECT 1 FROM runtime_events result WHERE result.invocation_id = call.invocation_id
             AND result.kind = 'model_observed' AND result.sequence > call.sequence AND result.sequence < interrupted.sequence
             AND json_extract(result.event_json, '$.fact.step_id') = json_extract(call.event_json, '$.fact.step_id')
             AND json_extract(result.event_json, '$.fact.event.kind') = 'provider_tool_result'
             AND json_extract(result.event_json, '$.fact.event.data.id') = json_extract(call.event_json, '$.fact.event.data.id')
             AND json_extract(result.event_json, '$.fact.event.data.name') = json_extract(call.event_json, '$.fact.event.data.name')))",
    ).bind(session).bind(current).bind(through as i64).fetch_one(&mut *connection).await?;
    if unknown_provider_effect {
        return Err(invalid(
            "Session contains an unresolved prior provider tool effect",
        ));
    }
    let broken_payload: bool = sqlx::query_scalar(
        "WITH runtime_events AS NOT MATERIALIZED (SELECT * FROM main.runtime_events WHERE sequence <= ?2)
         SELECT EXISTS(SELECT 1 FROM runtime_events e LEFT JOIN tool_result_payloads p ON p.event_id = e.event_id
         WHERE json_extract(e.event_json, '$.invocation.session_id') = ?1
         AND e.kind = 'tool_settled' AND json_extract(e.event_json, '$.fact.outcome.kind') = 'succeeded'
         AND (p.event_id IS NULL OR length(p.payload) != COALESCE(json_extract(e.event_json, '$.fact.outcome.raw.bytes'), 0)))",
    ).bind(session).bind(through as i64).fetch_one(connection).await?;
    if broken_payload {
        return Err(invalid("source tool payload binding is broken"));
    }
    Ok(())
}

pub(super) async fn closed_boundary(
    connection: &mut SqliteConnection,
    session: &str,
    through: u64,
) -> Result<(), StoreError> {
    let closed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE sequence = ? AND kind = 'invocation_ended'
         AND json_extract(event_json, '$.invocation.session_id') = ?)",
    ).bind(i64::try_from(through).map_err(|_| invalid("coverage overflow"))?).bind(session).fetch_one(connection).await?;
    if !closed {
        return Err(invalid("coverage must end at a closed Session boundary"));
    }
    Ok(())
}

/// A live opening is allowed; every covered model and tool operation must close.
pub(crate) async fn active_boundary(
    connection: &mut SqliteConnection,
    invocation: &str,
    through: u64,
) -> Result<(), StoreError> {
    execution_boundary(connection, invocation, through, false).await
}

pub(crate) async fn prune_boundary(
    connection: &mut SqliteConnection,
    invocation: &str,
    through: u64,
) -> Result<(), StoreError> {
    execution_boundary(connection, invocation, through, true).await
}

async fn execution_boundary(
    connection: &mut SqliteConnection,
    invocation: &str,
    through: u64,
    allow_summary_partial: bool,
) -> Result<(), StoreError> {
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events e WHERE e.invocation_id = ?1 AND e.sequence <= ?2 AND (
          (e.kind = 'tool_dispatched' AND NOT EXISTS(SELECT 1 FROM runtime_events t
             WHERE t.invocation_id = e.invocation_id AND t.operation_id = e.operation_id AND t.kind = 'tool_settled' AND t.sequence <= ?2))
          OR (e.kind = 'model_requested' AND NOT EXISTS(SELECT 1 FROM runtime_events t
             WHERE t.invocation_id = e.invocation_id AND t.operation_id = e.operation_id AND t.kind IN ('model_completed','model_interrupted') AND t.sequence <= ?2))
          OR (e.kind = 'model_completed' AND EXISTS(SELECT 1 FROM json_each(e.event_json, '$.fact.output.parts') p
             WHERE json_extract(p.value, '$.kind') = 'tool_call' AND json_extract(p.value, '$.call.provider_executed') = 0
             AND NOT EXISTS(SELECT 1 FROM runtime_events d WHERE d.invocation_id = e.invocation_id
               AND d.operation_id = e.operation_id || ':' || json_extract(p.value, '$.call.id')
               AND d.kind IN ('tool_dispatched','tool_rejected') AND d.sequence <= ?2)))
          OR (e.kind = 'model_interrupted'
             AND (json_extract(e.event_json, '$.fact.status') != 'retryable_failure'
               OR EXISTS(SELECT 1 FROM runtime_events barrier WHERE barrier.invocation_id=e.invocation_id
                 AND json_extract(barrier.event_json, '$.fact.step_id')=e.operation_id
                 AND barrier.kind='model_observed' AND barrier.sequence <= ?2
                 AND (json_extract(barrier.event_json, '$.fact.event.kind') IN ('provider_tool_result','finished')
                   OR (json_extract(barrier.event_json, '$.fact.event.kind')='tool_call'
                     AND json_extract(barrier.event_json, '$.fact.event.data.provider_executed')=1)
                   OR (json_extract(barrier.event_json, '$.fact.event.data.provider_options') IS NOT NULL
                     AND json_extract(barrier.event_json, '$.fact.event.data.provider_options') != '{}'))))
             AND NOT (?3 AND EXISTS(SELECT 1 FROM runtime_events r WHERE r.invocation_id=e.invocation_id AND r.operation_id=e.operation_id
               AND r.kind='model_requested' AND json_extract(r.event_json,'$.fact.purpose')='summary')
               AND NOT EXISTS(SELECT 1 FROM runtime_events o WHERE o.invocation_id=e.invocation_id
                 AND json_extract(o.event_json,'$.fact.step_id')=e.operation_id AND o.kind='model_observed'
                 AND json_extract(o.event_json,'$.fact.event.kind') IN ('tool_call','provider_tool_result')))
             AND EXISTS(SELECT 1 FROM runtime_events o
             WHERE o.invocation_id = e.invocation_id AND json_extract(o.event_json, '$.fact.step_id') = e.operation_id
             AND o.kind = 'model_observed' AND o.sequence <= ?2))
          OR (e.kind = 'model_completed' AND EXISTS(SELECT 1 FROM json_each(e.event_json, '$.fact.output.parts') p
             WHERE json_extract(p.value, '$.kind') = 'tool_call' AND json_extract(p.value, '$.call.provider_executed') = 1
             AND NOT EXISTS(SELECT 1 FROM json_each(e.event_json, '$.fact.output.parts') result
               WHERE json_extract(result.value, '$.kind') = 'tool_result'
                 AND json_extract(result.value, '$.id') = json_extract(p.value, '$.call.id')
                 AND json_extract(result.value, '$.name') = json_extract(p.value, '$.call.name'))))))",
    ).bind(invocation).bind(through as i64).bind(allow_summary_partial).fetch_one(connection).await?;
    if pending {
        return Err(invalid(
            "active source contains unresolved model or tool work",
        ));
    }
    Ok(())
}
