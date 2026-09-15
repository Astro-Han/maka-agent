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

use super::{invalid, safety, selection::Selection};
use crate::StoreError;
use maka_runtime::{
    context::CheckpointMode,
    event::{Fact, InvocationInput, RuntimeEvent},
};
use sqlx::SqliteConnection;

pub(super) async fn source_fence(
    connection: &mut SqliteConnection,
    selection: &Selection,
    opening: Option<&(i64, RuntimeEvent)>,
    mode: &CheckpointMode,
) -> Result<u64, StoreError> {
    let session = selection
        .session
        .as_deref()
        .ok_or_else(|| invalid("context requires a Session"))?;
    let Some((opened, event)) = opening else {
        if !matches!(mode, CheckpointMode::Standalone) {
            return Err(invalid(
                "automatic compaction requires a live model invocation",
            ));
        }
        let high = selection
            .high_water(connection, i64::MAX as u64 - 1)
            .await?;
        if high > 0 {
            safety::closed_boundary(connection, session, high).await?;
        }
        return Ok(high);
    };
    let Fact::InvocationOpened { input, .. } = &event.fact else {
        return Err(invalid("missing canonical opening"));
    };
    let before = match (mode, input) {
        (CheckpointMode::Standalone, InvocationInput::ContextCompact { .. })
        | (
            CheckpointMode::PreTurn,
            InvocationInput::Message { .. }
            | InvocationInput::Continuation { .. }
            | InvocationInput::Handoff { .. },
        ) => *opened,
        (
            CheckpointMode::MidTurn { anchor_event_id },
            InvocationInput::Message { .. }
            | InvocationInput::Continuation { .. }
            | InvocationInput::Handoff { .. },
        ) => {
            if anchor_event_id != &event.id {
                return Err(invalid(
                    "mid-turn anchor is not the exact canonical opening",
                ));
            }
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT MIN(sequence) FROM runtime_events WHERE invocation_id = ? AND kind = 'model_requested'
                 AND json_extract(event_json, '$.fact.purpose') = 'summary'",
            ).bind(&event.invocation.invocation_id).fetch_one(&mut *connection).await?.unwrap_or(i64::MAX)
        }
        _ => return Err(invalid("checkpoint mode does not match its opening")),
    };
    let high = selection.high_water(connection, before as u64 - 1).await?;
    if matches!(mode, CheckpointMode::MidTurn { .. }) {
        if high <= *opened as u64 {
            return Err(invalid(
                "mid-turn source has no completed work after its anchor",
            ));
        }
        safety::active_boundary(connection, &event.invocation.invocation_id, high).await?;
    } else if high > 0 {
        safety::closed_boundary(connection, session, high).await?;
    }
    Ok(high)
}

/// The summary attempt may repair its output, but never advance its source.
pub(super) async fn summary_span(
    connection: &mut SqliteConnection,
    event: &RuntimeEvent,
    through: u64,
) -> Result<(), StoreError> {
    let first: Option<i64> = sqlx::query_scalar(
        "SELECT MIN(sequence) FROM runtime_events WHERE invocation_id = ? AND kind = 'model_requested'
         AND (json_extract(event_json, '$.fact.purpose') = 'summary'
           OR (json_extract(event_json, '$.fact.purpose') IS NULL AND EXISTS(SELECT 1 FROM runtime_events o
             WHERE o.invocation_id = runtime_events.invocation_id AND o.kind = 'invocation_opened'
             AND json_extract(o.event_json, '$.fact.input.kind') = 'context_compact')))",
    ).bind(&event.invocation.invocation_id).fetch_one(&mut *connection).await?;
    let Some(first) = first else {
        return Ok(());
    };
    let work: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id = ?1 AND sequence > ?2 AND sequence < ?3
         AND (kind IN ('message_steered','tool_dispatched','tool_rejected','tool_settled')
           OR (kind = 'model_requested' AND COALESCE(json_extract(event_json, '$.fact.purpose'),
             (SELECT CASE json_extract(o.event_json, '$.fact.input.kind') WHEN 'context_compact' THEN 'summary' ELSE 'main' END
               FROM runtime_events o WHERE o.invocation_id = runtime_events.invocation_id AND o.kind = 'invocation_opened')) != 'summary')))",
    ).bind(&event.invocation.invocation_id).bind(first).bind(through as i64).fetch_one(connection).await?;
    if work {
        return Err(invalid(
            "message, main or tool work interleaved with summary repairs",
        ));
    }
    Ok(())
}
