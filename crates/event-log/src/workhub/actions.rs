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

use crate::{EventLog, StoreError, sequence_number};
use maka_runtime::event::{Fact, Invocation, RuntimeEvent, StoredEvent};
use sqlx::SqliteConnection;

impl EventLog {
    /// Root-global identity across delegation, continuation and observation facts.
    pub async fn workhub_action(&self, action: &str) -> Result<Option<StoredEvent>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(action)?;
        let action = action.to_owned();
        self.connection
            .run(move |connection| Box::pin(async move { read(connection, &action).await }))
            .await
    }
}

pub(super) async fn read(
    tx: &mut SqliteConnection,
    action: &str,
) -> Result<Option<StoredEvent>, StoreError> {
    let row: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT sequence, CASE WHEN length(CAST(event_json AS BLOB)) <= 1048576 THEN event_json END
         FROM runtime_events
         WHERE kind IN ('workhub_delegated', 'workhub_resume_observed', 'invocation_opened')
         AND CASE
            WHEN kind = 'workhub_delegated'
                THEN json_extract(event_json, '$.fact.delegation.action_id')
            WHEN kind = 'workhub_resume_observed'
                THEN json_extract(event_json, '$.fact.resume.action_id')
            WHEN kind = 'invocation_opened'
                AND json_extract(event_json, '$.fact.input.kind') = 'continuation'
                THEN json_extract(event_json, '$.fact.input.workhub_resume.action_id')
         END = ?",
    )
    .bind(action)
    .fetch_optional(tx)
    .await?;
    row.map(|(sequence, json)| {
        let event: RuntimeEvent = serde_json::from_str(&json.ok_or(StoreError::PrefixTooLarge)?)?;
        let id = match &event.fact {
            Fact::WorkhubDelegated { delegation } => {
                delegation
                    .validate(&event.invocation)
                    .map_err(super::invalid)?;
                &delegation.action_id
            }
            _ => {
                let resume = super::resume::validate_record(&event)?;
                &resume.action_id
            }
        };
        if id != action {
            return Err(super::invalid("WorkHub action identity changed"));
        }
        Ok(StoredEvent {
            sequence: sequence_number(sequence)?,
            event,
        })
    })
    .transpose()
}

pub(super) async fn require_coordinator(
    tx: &mut SqliteConnection,
    coordinator: &Invocation,
) -> Result<(), StoreError> {
    let source: Option<Option<String>> = sqlx::query_scalar(
        "SELECT CASE WHEN length(CAST(event_json AS BLOB)) <= 1048576 THEN event_json END
         FROM runtime_events o WHERE o.kind = 'invocation_opened' AND o.invocation_id = ?
         AND NOT EXISTS(SELECT 1 FROM runtime_events t WHERE t.invocation_id = o.invocation_id AND t.kind = 'invocation_ended')"
    ).bind(&coordinator.invocation_id).fetch_optional(tx).await?;
    let source: RuntimeEvent = serde_json::from_str(
        &source
            .ok_or_else(|| super::invalid("WorkHub source is no longer active"))?
            .ok_or(StoreError::PrefixTooLarge)?,
    )?;
    if source.invocation != *coordinator
        || coordinator.session_id != maka_runtime::workhub::COORDINATION_SESSION_ID
        || !matches!(
            source.fact,
            Fact::InvocationOpened {
                input: maka_runtime::input::InvocationInput::Message { .. },
                ..
            }
        )
    {
        return Err(super::invalid(
            "WorkHub source is not its authorized message",
        ));
    }
    Ok(())
}
