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

use crate::{
    StoreError,
    turns::{InvocationState, TurnBoundary},
};
use maka_runtime::model::TextKind;
use rusqlite::{Connection, functions::FunctionFlags};
use sqlx::Row;

/// Active stream identity and committed UTF-16 position, without transcript text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantStreamSeed {
    pub step_id: String,
    pub part_id: String,
    /// Canonical PartStarted event ID; provider IDs may repeat across steps.
    pub message_id: String,
    pub text_kind: TextKind,
    pub offset: u64,
}

pub(crate) fn register_function(connection: &Connection) -> Result<(), StoreError> {
    connection.create_scalar_function(
        "maka_utf16_length",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| Ok(context.get_raw(0).as_str()?.encode_utf16().count() as i64),
    )?;
    Ok(())
}

pub(super) async fn read(
    connection: &mut sqlx::SqliteConnection,
    root: Option<&TurnBoundary>,
) -> Result<Vec<AssistantStreamSeed>, StoreError> {
    let Some(root) = root else {
        return Ok(Vec::new());
    };
    if matches!(root.state, InvocationState::Ended { .. })
        || matches!(
            root.input,
            maka_runtime::input::InvocationInput::ContextCompact { .. }
        )
    {
        return Ok(Vec::new());
    }
    // Select only identities. Neither finished output bodies nor accumulated text
    // cross the SQL boundary. The existing invocation_sequence index scopes scans.
    let rows = sqlx::query(
        "SELECT s.sequence, json_extract(s.event_json, '$.fact.step_id'), s.event_id,
                json_extract(s.event_json, '$.fact.event.data.id'),
                json_extract(s.event_json, '$.fact.event.data.text_kind')
         FROM runtime_events s WHERE s.invocation_id = ?1 AND s.kind = 'model_observed'
         AND EXISTS (SELECT 1 FROM runtime_events request
             JOIN runtime_events opening ON opening.invocation_id = request.invocation_id
             AND opening.kind = 'invocation_opened'
             WHERE request.kind = 'model_requested' AND request.invocation_id = s.invocation_id
             AND request.operation_id = json_extract(s.event_json, '$.fact.step_id')
             AND json_extract(opening.event_json, '$.fact.input.kind') = 'message'
             AND COALESCE(json_extract(request.event_json, '$.fact.purpose'), 'main') = 'main')
         AND json_extract(s.event_json, '$.fact.event.kind') = 'part_started'
         AND NOT EXISTS (
             SELECT 1 FROM runtime_events e
             WHERE e.invocation_id = s.invocation_id
             AND json_extract(e.event_json, '$.fact.step_id') = json_extract(s.event_json, '$.fact.step_id')
             AND (e.kind IN ('model_interrupted', 'model_completed') OR
                 (e.sequence > s.sequence AND e.kind = 'model_observed'
                  AND json_extract(e.event_json, '$.fact.event.kind') = 'part_started'
                  AND json_extract(e.event_json, '$.fact.event.data.id') =
                      json_extract(s.event_json, '$.fact.event.data.id'))))
         ORDER BY s.sequence LIMIT 129",
    ).bind(&root.invocation.invocation_id).fetch_all(&mut *connection).await?;
    let mut result = Vec::new();
    for row in rows {
        if result.len() == 128 {
            return Err(StoreError::PrefixTooLarge);
        }
        let sequence: i64 = row.try_get(0)?;
        let step_id: String = row.try_get(1)?;
        let part_id: String = row.try_get(3)?;
        let text_kind = match row.try_get::<String, _>(4)?.as_str() {
            "text" => TextKind::Text,
            "thinking" => TextKind::Thinking,
            _ => {
                return Err(StoreError::InvalidTransition(
                    "invalid stored text kind".into(),
                ));
            }
        };
        let offset: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(maka_utf16_length(json_extract(event_json, '$.fact.event.data.text'))), 0)
             FROM runtime_events WHERE invocation_id = ?1 AND json_extract(event_json, '$.fact.step_id') = ?2
             AND sequence > ?3 AND kind = 'model_observed'
             AND json_extract(event_json, '$.fact.event.kind') = 'part_delta'
             AND json_extract(event_json, '$.fact.event.data.id') = ?4",
        ).bind(&root.invocation.invocation_id).bind(&step_id).bind(sequence).bind(&part_id)
            .fetch_one(&mut *connection).await?;
        result.push(AssistantStreamSeed {
            step_id,
            part_id,
            message_id: row.try_get(2)?,
            text_kind,
            offset: u64::try_from(offset).map_err(|_| StoreError::PrefixTooLarge)?,
        });
    }
    Ok(result)
}
