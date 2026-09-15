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

use super::evidence;
use crate::StoreError;
use maka_presentation::ProjectionError;
use maka_runtime::event::StoredEvent;
use sqlx::SqliteConnection;
use std::collections::BTreeSet;

/// Reconstruct one tool boundary, its T1 ancestry and its accepted provider step.
/// Other steps, sibling outcomes and large unrelated tool payloads are excluded.
pub(super) async fn selected(
    tx: &mut SqliteConnection,
    invocation: &str,
    through: i64,
) -> Result<Vec<StoredEvent>, StoreError> {
    let (operation, kind): (String, String) = sqlx::query_as(
        "SELECT operation_id, kind FROM runtime_events WHERE sequence = ? AND invocation_id = ?",
    )
    .bind(through)
    .bind(invocation)
    .fetch_one(&mut *tx)
    .await?;
    let mut ids = BTreeSet::from([through]);
    let mut current = operation;
    let mut before = through;
    let mut rejected = kind == "tool_rejected";
    let step = loop {
        if ids.len() > 33 {
            return Err(StoreError::PrefixTooLarge);
        }
        let header: Option<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT sequence, json_extract(event_json, '$.fact.call.origin.kind'),
                json_extract(event_json, '$.fact.call.origin.step_id'),
                json_extract(event_json, '$.fact.call.origin.parent_operation_id')
             FROM runtime_events
             WHERE invocation_id = ?1 AND operation_id = ?2 AND operation_id IS NOT NULL
               AND kind = ?3 AND sequence <= ?4 LIMIT 1",
        )
        .bind(invocation)
        .bind(&current)
        .bind(if rejected {
            "tool_rejected"
        } else {
            "tool_dispatched"
        })
        .bind(before)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((sequence, origin, step, parent)) = header else {
            return Err(
                ProjectionError::Invalid("tool presentation has no dispatch provenance").into(),
            );
        };
        ids.insert(sequence);
        if ids.len() > 33 {
            return Err(StoreError::PrefixTooLarge);
        }
        match origin.as_str() {
            "provider" => {
                break Some(step.ok_or(ProjectionError::Invalid("missing provider step"))?);
            }
            "standalone" => break None,
            "code_mode" => {
                current = parent.ok_or(ProjectionError::Invalid("missing parent operation"))?;
                before = sequence - 1;
                rejected = false;
            }
            _ => return Err(ProjectionError::Invalid("unknown tool origin").into()),
        }
    };
    if let Some(step) = &step {
        let main: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_events request
             JOIN runtime_events opening ON opening.invocation_id = request.invocation_id
             AND opening.kind = 'invocation_opened'
             WHERE request.invocation_id = ?1 AND request.operation_id = ?2
             AND request.kind = 'model_requested'
             AND json_extract(opening.event_json, '$.fact.input.kind') IN ('message', 'continuation', 'handoff')
             AND COALESCE(json_extract(request.event_json, '$.fact.purpose'), 'main') = 'main')",
        )
        .bind(invocation)
        .bind(step)
        .fetch_one(&mut *tx)
        .await?;
        if !main {
            return Err(ProjectionError::Invalid("tool belongs to a non-main step").into());
        }
    }
    let ids = serde_json::to_string(&ids)?;
    let query = sqlx::query(
        "SELECT sequence, length(CAST(event_json AS BLOB)) FROM runtime_events
         WHERE invocation_id = ?1 AND sequence <= ?3 AND (
            kind = 'invocation_opened' OR sequence IN (SELECT value FROM json_each(?4))
            OR (?2 IS NOT NULL AND (
                (kind IN ('model_requested', 'model_completed') AND operation_id = ?2)
                OR (kind = 'model_observed' AND json_extract(event_json, '$.fact.step_id') = ?2)
            )))
         ORDER BY sequence LIMIT ?5",
    )
    .bind(invocation)
    .bind(step)
    .bind(through)
    .bind(ids)
    .bind((evidence::MAX_EVENTS + 1) as i64);
    evidence::read(tx, query).await
}
