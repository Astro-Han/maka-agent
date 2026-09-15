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

use super::{high_water, invalid};
use crate::{EventLog, StoreError, sessions};
use futures_util::TryStreamExt;
use maka_runtime::{event::Invocation, model::TextKind};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, Row};

/// Delivery projection of committed facts, never model replay material.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreStreamEvent {
    pub sequence: u64,
    pub id: String,
    pub invocation: Invocation,
    pub root_run_id: String,
    pub recorded_at: std::time::SystemTime,
    pub fact: StreamFact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamFact {
    InvocationOpened,
    WorkhubDelegated,
    MessageSteered,
    PartStarted {
        step_id: String,
        part_id: String,
        text_kind: TextKind,
    },
    PartDelta {
        step_id: String,
        part_id: String,
        text: String,
    },
    PartFinished {
        step_id: String,
        part_id: String,
    },
    StepEnded {
        step_id: String,
        failed: bool,
        #[serde(default)]
        interrupted: Vec<String>,
    },
    InvocationEnded {
        failed: bool,
        #[serde(default)]
        interrupted: Vec<String>,
    },
    ToolDispatched {
        operation_id: String,
        name: String,
    },
    ToolRejected {
        operation_id: String,
        name: String,
    },
    ToolSettled {
        operation_id: String,
        outcome: ToolSettlement,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSettlement {
    Succeeded,
    Failed,
}

#[derive(Debug)]
pub struct StreamEventPage {
    pub events: Vec<StoreStreamEvent>,
    /// Latest Session-owned transcript boundary in this page's consumed range.
    pub session_boundary: Option<u64>,
    pub through_sequence: u64,
    /// Continue strictly after this sequence; None means the fence is exhausted.
    pub next_after: Option<u64>,
}

impl EventLog {
    /// Read only stream delivery fields in (after, through]. Unrelated facts and
    /// provider payloads consume no page budget. The byte budget counts projected
    /// JSON, including identities and escaping; an oversized first item is an error.
    pub async fn session_stream_events(
        &self,
        session_id: &str,
        after: u64,
        through: u64,
        max_events: usize,
        max_bytes: usize,
    ) -> Result<StreamEventPage, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session_id)?;
        if after > through || max_events == 0 || max_events > 512 || max_bytes == 0 {
            return Err(invalid("invalid stream page bounds"));
        }
        let after = i64::try_from(after).map_err(|_| invalid("invalid stream cursor"))?;
        let fence = i64::try_from(through).map_err(|_| invalid("invalid stream fence"))?;
        let session_id = session_id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut transaction = connection.begin().await?;
                    if through > high_water(&mut transaction).await? {
                        return Err(invalid("stream fence is beyond committed history"));
                    }
                    let mut events: Vec<StoreStreamEvent> = Vec::new();
                    let mut bytes = 0usize;
                    let mut next_after = None;
                    {
                        let mut rows = sqlx::query(STREAM_EVENTS)
                            .bind(&session_id)
                            .bind(after)
                            .bind(fence)
                            .bind(max_events as i64 + 1)
                            .fetch(&mut *transaction);
                        while let Some(row) = rows.try_next().await? {
                            // Check projected JSON before decoding identities or delta text.
                            // No complete RuntimeEvent is deserialized.
                            let json: &str = row.try_get(0)?;
                            if events.len() == max_events
                                || json.len() > max_bytes.saturating_sub(bytes)
                            {
                                let last = events.last().ok_or(StoreError::PrefixTooLarge)?;
                                next_after = Some(last.sequence);
                                break;
                            }
                            let event: StoreStreamEvent = serde_json::from_str(json)?;
                            if event.invocation.session_id != session_id {
                                return Err(invalid("stored stream scope changed"));
                            }
                            bytes += json.len();
                            events.push(event);
                        }
                    }
                    // Only failed boundaries need their canonical presentation verdict.
                    // Reuse its bounded reconstruction; provider metadata never enters delivery.
                    let mut enriched = Vec::new();
                    bytes = 0;
                    for mut event in events {
                        let step = match &event.fact {
                            StreamFact::StepEnded {
                                step_id,
                                failed: true,
                                ..
                            } => Some(Some(step_id.as_str())),
                            StreamFact::InvocationEnded { failed: true, .. } => Some(None),
                            _ => None,
                        };
                        if let Some(step) = step {
                            let ids = crate::transcript::interrupted_messages(
                                &mut transaction,
                                &event.invocation.invocation_id,
                                step,
                                event.sequence,
                            )
                            .await?;
                            match &mut event.fact {
                                StreamFact::StepEnded { interrupted, .. }
                                | StreamFact::InvocationEnded { interrupted, .. } => {
                                    *interrupted = ids
                                }
                                _ => unreachable!(),
                            }
                        }
                        let size = serde_json::to_vec(&event)?.len();
                        if size > max_bytes.saturating_sub(bytes) {
                            let last: &StoreStreamEvent =
                                enriched.last().ok_or(StoreError::PrefixTooLarge)?;
                            next_after = Some(last.sequence);
                            break;
                        }
                        bytes += size;
                        enriched.push(event);
                    }
                    let consumed = next_after.unwrap_or(through);
                    let session_boundary: Option<i64> = sqlx::query_scalar(
                        "SELECT MAX(sequence) FROM session_events
                         WHERE json_extract(event_json, '$.session_id') = ?1
                           AND sequence > ?2 AND sequence <= ?3",
                    )
                    .bind(&session_id)
                    .bind(after)
                    .bind(i64::try_from(consumed).map_err(|_| invalid("invalid stream boundary"))?)
                    .fetch_one(&mut *transaction)
                    .await?;
                    let session_boundary =
                        session_boundary.map(crate::sequence_number).transpose()?;
                    transaction.commit().await?;
                    Ok(StreamEventPage {
                        events: enriched,
                        session_boundary,
                        through_sequence: through,
                        next_after,
                    })
                })
            })
            .await
    }
}

// CASE evaluates only the selected arm: accepted model output, tool input/output,
// and provider options are never included in the selected delivery value.
const STREAM_EVENTS: &str = "
WITH events AS MATERIALIZED (
SELECT json_object(
    'sequence', sequence, 'id', event_id,
    'recorded_at', json_extract(event_json, '$.recorded_at'),
    'invocation', json_object(
        'session_id', json_extract(event_json, '$.invocation.session_id'),
        'turn_id', json_extract(event_json, '$.invocation.turn_id'),
        'run_id', json_extract(event_json, '$.invocation.run_id'),
        'invocation_id', invocation_id),
    'fact', json(CASE
        WHEN kind IN ('tool_dispatched', 'tool_rejected') THEN
            json_object('kind', kind, 'operation_id', operation_id,
                'name', json_extract(event_json, '$.fact.name'))
        WHEN kind = 'tool_settled' THEN
            json_object('kind', kind, 'operation_id', operation_id,
                'outcome', json_extract(event_json, '$.fact.outcome.kind'))
        WHEN kind IN ('invocation_opened', 'message_steered', 'workhub_delegated') THEN json_object('kind', kind)
        WHEN kind = 'invocation_ended' THEN json_object('kind', kind,
            'failed', json(CASE WHEN json_extract(event_json, '$.fact.outcome.kind') = 'failed' THEN 'true' ELSE 'false' END))
        WHEN kind IN ('model_completed', 'model_interrupted') THEN
            json_object('kind', 'step_ended',
                'step_id', json_extract(event_json, '$.fact.step_id'),
                'failed', json(CASE WHEN kind = 'model_interrupted' AND json_extract(event_json, '$.fact.status') != 'cancelled' THEN 'true' ELSE 'false' END))
        WHEN json_extract(event_json, '$.fact.event.kind') = 'part_started' THEN
            json_object('kind', 'part_started',
                'step_id', json_extract(event_json, '$.fact.step_id'),
                'part_id', json_extract(event_json, '$.fact.event.data.id'),
                'text_kind', json_extract(event_json, '$.fact.event.data.text_kind'))
        WHEN json_extract(event_json, '$.fact.event.kind') = 'part_delta' THEN
            json_object('kind', 'part_delta',
                'step_id', json_extract(event_json, '$.fact.step_id'),
                'part_id', json_extract(event_json, '$.fact.event.data.id'),
                'text', json_extract(event_json, '$.fact.event.data.text'))
        ELSE json_object('kind', 'part_finished',
                'step_id', json_extract(event_json, '$.fact.step_id'),
                'part_id', json_extract(event_json, '$.fact.event.data.id'))
    END)) AS projected, invocation_id AS owner, sequence
FROM runtime_events
WHERE sequence > ?2 AND sequence <= ?3
AND json_extract(event_json, '$.invocation.session_id') = ?1
AND (kind = 'invocation_ended' OR NOT EXISTS (
    SELECT 1 FROM runtime_events opening
    WHERE opening.invocation_id = runtime_events.invocation_id
      AND opening.kind = 'invocation_opened'
      AND json_extract(opening.event_json, '$.fact.input.kind') = 'context_compact'))
AND (kind NOT IN ('model_observed', 'model_completed', 'model_interrupted') OR
     EXISTS (SELECT 1 FROM runtime_events request
             JOIN runtime_events opening ON opening.invocation_id = request.invocation_id
             AND opening.kind = 'invocation_opened'
             WHERE request.kind = 'model_requested' AND request.invocation_id = runtime_events.invocation_id
             AND request.operation_id = json_extract(runtime_events.event_json, '$.fact.step_id')
             AND json_extract(opening.event_json, '$.fact.input.kind') IN ('message', 'continuation', 'handoff')
             AND COALESCE(json_extract(request.event_json, '$.fact.purpose'), 'main') = 'main'))
AND (kind IN ('invocation_opened', 'message_steered', 'invocation_ended', 'model_completed', 'model_interrupted',
             'tool_dispatched', 'tool_rejected', 'tool_settled', 'workhub_delegated')
     OR (kind = 'model_observed'
         AND json_extract(event_json, '$.fact.event.kind') IN
             ('part_started', 'part_delta', 'part_finished')))
ORDER BY sequence LIMIT ?4
), roots AS MATERIALIZED (
 SELECT opening.invocation_id,
   CASE WHEN json_extract(opening.event_json,'$.fact.input.kind')='handoff'
     THEN json_extract(opening.event_json,'$.fact.input.pause.intent.root_run_id')
     ELSE json_extract(opening.event_json,'$.invocation.run_id') END AS run_id
 FROM runtime_events opening
 JOIN (SELECT DISTINCT owner FROM events) owners ON owners.owner=opening.invocation_id
 WHERE opening.kind='invocation_opened'
)
SELECT json_set(events.projected, '$.root_run_id', roots.run_id)
FROM events LEFT JOIN roots ON roots.invocation_id=events.owner
ORDER BY events.sequence";
