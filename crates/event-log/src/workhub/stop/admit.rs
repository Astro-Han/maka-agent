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

use super::{StopIntent, StopRecord, StopRequest, StopResolution, invalid, subject};
use crate::{StoreError, message_resolution::MessageExecution};
use maka_runtime::session_event::{SessionEvent, SessionFact};
use sqlx::SqliteConnection;

pub(super) async fn apply(
    tx: &mut SqliteConnection,
    request: StopRequest,
) -> Result<(StopRecord, u64), StoreError> {
    if super::super::actions::read(tx, &request.action_id)
        .await?
        .is_some()
    {
        return Err(invalid(
            "WorkHub action already belongs to another operation",
        ));
    }
    let content = super::super::actions::require_coordinator(tx, &request.source, None).await?;
    let target: Option<(bool, Option<String>)> = sqlx::query_as(
        "SELECT archived, CASE
            WHEN json_type(configuration, '$.name') = 'text'
             AND length(CAST(json_extract(configuration, '$.name') AS BLOB)) <= 4096
            THEN json_extract(configuration, '$.name') END
            FROM session_control WHERE id = ?",
    )
    .bind(&request.target_session_id)
    .fetch_optional(&mut *tx)
    .await?;
    let target_session_name = match target {
        None => return Err(StoreError::SessionNotFound),
        Some((true, _)) => return Err(invalid("WorkHub stop target is archived")),
        Some((false, name)) => {
            name.ok_or_else(|| invalid("WorkHub stop target name is missing"))?
        }
    };
    let (delegation, work) = subject::select(tx, &request.target_session_id).await?;
    super::super::assignment::require_unclaimed(tx, &delegation.action_id).await?;
    let claimed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workhub_stops WHERE delegation_action_id = ? AND (resolution_json IS NULL OR json_extract(resolution_json, '$.outcome') != 'not_owned'))")
        .bind(delegation.action_id.as_str()).fetch_one(&mut *tx).await?;
    if claimed {
        return Err(invalid("WorkHub delegation already has a stop claim"));
    }
    let mut intent = StopIntent {
        request,
        delegation_action_id: delegation.action_id.clone(),
        owner: None,
    };
    let result = match work {
        MessageExecution::Pending => {
            let message = delegation.target_message_id();
            // Consumption, cancellation and command publication serialize in this transaction.
            sqlx::query("INSERT INTO message_cancellations(session_id, message_id, cancellation_id) VALUES (?, ?, ?)")
                .bind(&delegation.target.session_id).bind(&message).bind(intent.request.action_id.as_str())
                .execute(&mut *tx).await?;
            sqlx::query("DELETE FROM message_admissions WHERE session_id = ? AND message_id = ?")
                .bind(&delegation.target.session_id)
                .bind(&message)
                .execute(&mut *tx)
                .await?;
            crate::message_queue::bump(tx, &delegation.target.session_id).await?;
            Some(StopResolution::CancelledPending)
        }
        MessageExecution::Cancelled => Some(StopResolution::AlreadyTerminal {
            target_turn_id: None,
        }),
        MessageExecution::Owned(boundary) => {
            let terminal = boundary.state.terminal_outcome().is_some();
            intent.owner = Some(boundary.invocation.clone());
            terminal.then_some(StopResolution::AlreadyTerminal {
                target_turn_id: Some(boundary.invocation.turn_id),
            })
        }
        MessageExecution::Shared(boundary) => Some(StopResolution::NotOwned {
            target_turn_id: boundary.invocation.turn_id,
        }),
        MessageExecution::Missing => {
            return Err(invalid(
                "WorkHub delegation has no recoverable Message owner",
            ));
        }
    };
    let mut sequence = super::super::control::append(
        tx,
        &SessionEvent::workhub(
            intent.request.source.turn_id.clone(),
            SessionFact::WorkhubStopRequested {
                intent: Box::new(intent.clone()),
                target_session_name,
                user_text: content.text,
            },
        ),
    )
    .await?;
    if let Some(resolution) = &result {
        sequence = super::super::control::append(
            tx,
            &SessionEvent::workhub(
                intent.request.source.turn_id.clone(),
                SessionFact::WorkhubStopResolved {
                    action_id: intent.request.action_id.clone(),
                    resolution: resolution.clone(),
                },
            ),
        )
        .await?;
    }
    Ok((
        StopRecord {
            intent,
            resolution: result,
        },
        sequence,
    ))
}
