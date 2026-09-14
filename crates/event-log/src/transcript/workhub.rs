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

use crate::{StoreError, sequence_number};
use maka_runtime::{
    event::Fact,
    session_event::{SessionEvent, SessionFact},
};
use sqlx::SqliteConnection;

pub(super) async fn project(
    tx: &mut SqliteConnection,
    sequence: i64,
) -> Result<maka_presentation::Row, StoreError> {
    let json: Option<String> = sqlx::query_scalar(
        "SELECT CASE WHEN length(CAST(event_json AS BLOB)) <= 131072 THEN event_json END
         FROM session_events WHERE sequence = ?",
    )
    .bind(sequence)
    .fetch_one(&mut *tx)
    .await?;
    let event: SessionEvent = serde_json::from_str(&json.ok_or(StoreError::PrefixTooLarge)?)?;
    event
        .validate()
        .map_err(|reason| StoreError::InvalidTransition(reason.into()))?;
    let intent = match &event.fact {
        SessionFact::WorkhubStopRequested { intent, .. } => intent.as_ref().clone(),
        SessionFact::WorkhubStopResolved { action_id, .. } => {
            crate::workhub::stop::read(tx, action_id)
                .await?
                .ok_or_else(|| {
                    StoreError::InvalidTransition("WorkHub stop intent is missing".into())
                })?
                .intent
        }
    };
    let assigned = crate::workhub::actions::read(tx, &intent.delegation_action_id)
        .await?
        .ok_or_else(|| StoreError::InvalidTransition("WorkHub delegation is missing".into()))?;
    let Fact::WorkhubDelegated { delegation } = &assigned.event.fact else {
        return Err(StoreError::InvalidTransition(
            "WorkHub stop has no delegation".into(),
        ));
    };
    if delegation.target.session_id != intent.request.target_session_id {
        return Err(StoreError::InvalidTransition(
            "WorkHub stop target changed".into(),
        ));
    }
    Ok(maka_presentation::workhub::stop(
        sequence_number(sequence)?,
        &event,
        &intent,
        &assigned.event.id,
        &delegation.target_message_id(),
    )?)
}
