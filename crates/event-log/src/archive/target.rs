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

use super::ArchiveError;
use crate::{StoreError, sequence_number};
use maka_runtime::{
    event::Invocation,
    tool_call::{ToolCallIdentity, ToolOrigin},
    tool_output::DurableToolProjection,
};
use sqlx::{Row, SqliteConnection};

pub(super) struct Target {
    pub sequence: u64,
    pub event_id: String,
    pub invocation: Invocation,
    pub call: ToolCallIdentity,
    pub name: String,
    pub step_sequence: u64,
    pub projection: DurableToolProjection,
}

pub(super) async fn read(
    connection: &mut SqliteConnection,
    session: &str,
    id: &str,
) -> Result<Option<Target>, StoreError> {
    let row = sqlx::query(
        "SELECT t.sequence, t.event_id, t.operation_id, json_extract(t.event_json, '$.invocation') AS ti,
         json_extract(d.event_json, '$.invocation') AS di, json_extract(d.event_json, '$.fact.call') AS call,
         json_extract(d.event_json, '$.fact.name') AS name,
         json_extract(t.event_json, '$.fact.outcome.kind') AS outcome,
         CASE WHEN length(CAST(json_extract(t.event_json, '$.fact.outcome.model_projection') AS BLOB)) <= 2097152
           THEN json_extract(t.event_json, '$.fact.outcome.model_projection') END AS projection,
         CASE WHEN length(CAST(json_extract(t.event_json, '$.fact.outcome.message') AS BLOB)) <= 262144
           THEN json_extract(t.event_json, '$.fact.outcome.message') END AS message,
         c.sequence AS completed,
         (json_extract(t.event_json,'$.id')=t.event_id
           AND json_extract(t.event_json,'$.invocation.invocation_id')=t.invocation_id
           AND json_extract(t.event_json,'$.fact.operation_id')=t.operation_id
           AND json_extract(t.event_json,'$.fact.kind')='tool_settled'
           AND json_extract(d.event_json,'$.id')=d.event_id
           AND json_extract(d.event_json,'$.fact.operation_id')=d.operation_id
           AND json_extract(d.event_json,'$.fact.kind')='tool_dispatched'
           AND json_extract(c.event_json,'$.invocation')=json_extract(t.event_json,'$.invocation')
           AND json_extract(c.event_json,'$.fact.kind')='model_completed'
           AND json_extract(c.event_json,'$.fact.step_id')=c.operation_id
           AND c.sequence<d.sequence AND d.sequence<t.sequence) AS identities
         FROM runtime_events t LEFT JOIN runtime_events d ON d.invocation_id = t.invocation_id
           AND d.operation_id = t.operation_id AND d.kind = 'tool_dispatched'
         LEFT JOIN runtime_events c ON c.invocation_id = t.invocation_id AND c.kind = 'model_completed'
           AND c.operation_id = json_extract(d.event_json, '$.fact.call.origin.step_id')
         WHERE t.event_id = ? AND t.kind = 'tool_settled' AND json_extract(t.event_json, '$.invocation.session_id') = ?",
    ).bind(id).bind(session).fetch_optional(&mut *connection).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let decoded = (|| -> Option<Target> {
        if !row.try_get::<bool, _>("identities").ok()? {
            return None;
        }
        let invocation: Invocation =
            serde_json::from_str(row.try_get::<&str, _>("ti").ok()?).ok()?;
        let dispatch: Invocation = serde_json::from_str(row.try_get::<&str, _>("di").ok()?).ok()?;
        let call: ToolCallIdentity =
            serde_json::from_str(row.try_get::<&str, _>("call").ok()?).ok()?;
        if invocation != dispatch
            || invocation.session_id != session
            || !matches!(call.origin, ToolOrigin::Provider { .. })
        {
            return None;
        }
        let outcome: &str = row.try_get("outcome").ok()?;
        let projection = match outcome {
            "succeeded" => serde_json::from_str(row.try_get::<&str, _>("projection").ok()?).ok()?,
            "failed" => DurableToolProjection::Text {
                text: row.try_get("message").ok()?,
            },
            _ => return None,
        };
        Some(Target {
            sequence: sequence_number(row.try_get("sequence").ok()?).ok()?,
            event_id: row.try_get("event_id").ok()?,
            invocation,
            call,
            name: row.try_get("name").ok()?,
            step_sequence: sequence_number(row.try_get("completed").ok()?).ok()?,
            projection,
        })
    })()
    .ok_or(ArchiveError::Corrupt)?;
    let ToolOrigin::Provider { step_id } = &decoded.call.origin else {
        unreachable!()
    };
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events c, json_each(c.event_json, '$.fact.output.parts') p
         WHERE c.invocation_id = ? AND c.operation_id = ? AND c.kind = 'model_completed'
           AND json_extract(p.value, '$.kind') = 'tool_call' AND json_extract(p.value, '$.call.id') = ?
           AND json_extract(p.value, '$.call.name') = ? AND json_extract(p.value, '$.call.provider_executed') = 0)",
    ).bind(&decoded.invocation.invocation_id).bind(step_id).bind(&decoded.call.tool_call_id).bind(&decoded.name)
        .fetch_one(connection).await?;
    if !valid || decoded.step_sequence >= decoded.sequence {
        return Err(ArchiveError::Corrupt.into());
    }
    Ok(Some(decoded))
}
