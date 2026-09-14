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

use super::{MAX_SAFE_INTEGER, advance_catalog, invalid, number};
use crate::StoreError;
use maka_runtime::event::TerminalStatus;
use sqlx::{Row, SqliteConnection};
#[derive(Clone, Debug, PartialEq)]
pub struct SessionExecution {
    pub turn_id: String,
    pub state: SessionExecutionState,
    pub last_message: Option<CatalogMessage>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogMessage {
    pub recorded_at: u64,
    pub preview: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionExecutionState {
    Live {
        recorded_at: u64,
    },
    Ended {
        status: TerminalStatus,
        recorded_at: u64,
    },
}

pub(super) async fn read(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<Option<SessionExecution>, StoreError> {
    let opening = sqlx::query(
        "SELECT invocation_id, json_extract(event_json, '$.invocation.turn_id'),
                catalog_time(json_extract(event_json, '$.recorded_at'))
         FROM runtime_events INDEXED BY catalog_message_facts
         WHERE kind IN ('invocation_opened', 'model_completed') AND kind = 'invocation_opened'
         AND json_extract(event_json, '$.invocation.session_id') = ?
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(opening) = opening else {
        return Ok(None);
    };
    let invocation_id: String = opening.try_get(0)?;
    let turn_id = opening.try_get(1)?;
    let opened_at = number(&opening, 2)?;
    let terminal = sqlx::query(
        "SELECT json_extract(event_json, '$.fact.outcome.kind'),
                catalog_time(json_extract(event_json, '$.recorded_at')) FROM runtime_events
                INDEXED BY invocation_boundary
         WHERE invocation_id = ? AND kind IN ('invocation_opened', 'invocation_ended')
         AND kind = 'invocation_ended' ORDER BY sequence DESC LIMIT 1",
    )
    .bind(&invocation_id)
    .fetch_optional(&mut *connection)
    .await?;
    let terminal = terminal
        .map(|row| -> Result<_, StoreError> {
            let kind: String = row.try_get(0)?;
            let recorded_at = number(&row, 1)?;
            Ok(SessionExecutionState::Ended {
                recorded_at,
                status: match kind.as_str() {
                    "completed" | "context_compact_finished" => Ok(TerminalStatus::Completed),
                    "failed" => Ok(TerminalStatus::Failed),
                    "cancelled" => Ok(TerminalStatus::Cancelled),
                    _ => Err(invalid("unknown stored invocation outcome")),
                }?,
            })
        })
        .transpose()?;
    let last_message = sqlx::query(
        "SELECT message_at, (
            SELECT preview FROM catalog_messages INDEXED BY catalog_latest_preview
            WHERE session_id = ?1 AND preview IS NOT NULL
            ORDER BY message_at DESC, sequence DESC, ordinal DESC LIMIT 1
         ) FROM catalog_messages INDEXED BY catalog_latest_message WHERE session_id = ?1
         ORDER BY message_at DESC, sequence DESC, ordinal DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    .map(|row| -> Result<_, StoreError> {
        Ok(CatalogMessage {
            recorded_at: number(&row, 0)?,
            preview: row.try_get(1)?,
        })
    })
    .transpose()?;
    Ok(Some(SessionExecution {
        turn_id,
        state: terminal.unwrap_or(SessionExecutionState::Live {
            recorded_at: opened_at,
        }),
        last_message,
    }))
}

pub(crate) async fn advance_execution(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<(), StoreError> {
    let changed = sqlx::query(
        "UPDATE session_control SET revision = revision + 1 WHERE id = ? AND revision < ?",
    )
    .bind(id)
    .bind(MAX_SAFE_INTEGER as i64)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    if changed == 1 {
        advance_catalog(connection).await?;
    } else if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM session_control WHERE id = ?)",
    )
    .bind(id)
    .fetch_one(&mut *connection)
    .await?
    {
        return Err(invalid("session revision exhausted"));
    }
    Ok(())
}
