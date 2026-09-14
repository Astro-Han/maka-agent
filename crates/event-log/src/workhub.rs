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

use crate::{EventLog, StoreError, message_admissions::PendingMessageAdmission, sequence_number};
use maka_runtime::{
    event::{Fact, RuntimeEvent, StoredEvent},
    input::InvocationInput,
};
use sqlx::SqliteConnection;

mod candidates;
mod create;
pub use candidates::activity_at;

impl EventLog {
    /// Root-global action identity survives both source termination and Host epochs.
    pub async fn workhub_action(&self, action: &str) -> Result<Option<StoredEvent>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(action)?;
        let action = action.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let row: Option<(i64, Option<String>)> = sqlx::query_as(
                        "SELECT sequence, CASE WHEN length(CAST(event_json AS BLOB)) <= 1048576
                 THEN event_json END FROM runtime_events WHERE kind = 'workhub_delegated'
                 AND json_extract(event_json, '$.fact.delegation.action_id') = ?",
                    )
                    .bind(&action)
                    .fetch_optional(connection)
                    .await?;
                    row.map(|(sequence, json)| {
                        let event: RuntimeEvent =
                            serde_json::from_str(&json.ok_or(StoreError::PrefixTooLarge)?)?;
                        let Fact::WorkhubDelegated { delegation } = &event.fact else {
                            return Err(invalid("invalid WorkHub action index"));
                        };
                        delegation.validate(&event.invocation).map_err(invalid)?;
                        if delegation.action_id != action {
                            return Err(invalid("WorkHub action identity changed"));
                        }
                        Ok(StoredEvent {
                            sequence: sequence_number(sequence)?,
                            event,
                        })
                    })
                    .transpose()
                })
            })
            .await
    }
}

/// The pending target and the action fact share append's transaction. Exact
/// event replay returns before this hook, so it never recreates consumed work.
pub(crate) async fn apply(
    tx: &mut SqliteConnection,
    event: &RuntimeEvent,
) -> Result<(), StoreError> {
    let Fact::WorkhubDelegated { delegation } = &event.fact else {
        return Ok(());
    };
    delegation.validate(&event.invocation).map_err(invalid)?;
    let json: Option<Option<String>> = sqlx::query_scalar(
        "SELECT CASE WHEN length(CAST(event_json AS BLOB)) <= 1048576 THEN event_json END
         FROM runtime_events WHERE event_id = ? AND kind = 'invocation_opened'",
    )
    .bind(&delegation.source_message_event_id)
    .fetch_optional(&mut *tx)
    .await?;
    let source: RuntimeEvent = serde_json::from_str(
        &json
            .ok_or_else(|| invalid("WorkHub source message is missing"))?
            .ok_or(StoreError::PrefixTooLarge)?,
    )?;
    if source.invocation != event.invocation {
        return Err(invalid(
            "WorkHub action cannot borrow another Turn's user authority",
        ));
    }
    let InvocationInput::Message { content, .. } = (match source.fact {
        Fact::InvocationOpened { input, .. } => input,
        _ => return Err(invalid("invalid WorkHub source opening")),
    }) else {
        return Err(invalid("WorkHub action requires a user message"));
    };
    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS(
         SELECT 1 FROM runtime_events WHERE invocation_id = ?1
         UNION ALL SELECT 1 FROM runtime_events WHERE kind = 'invocation_opened'
           AND json_extract(event_json, '$.invocation.session_id') = ?2
           AND (json_extract(event_json, '$.invocation.run_id') = ?3
             OR json_extract(event_json, '$.invocation.turn_id') = ?4)
         UNION ALL SELECT 1 FROM message_admissions
           WHERE json_extract(record_json, '$.source.disposition') = 'turn_started'
           AND json_extract(record_json, '$.invocation.invocation_id') = ?1
         UNION ALL SELECT 1 FROM message_admissions WHERE session_id = ?2
           AND json_extract(record_json, '$.source.disposition') = 'turn_started'
           AND (json_extract(record_json, '$.invocation.run_id') = ?3
             OR json_extract(record_json, '$.invocation.turn_id') = ?4))",
    )
    .bind(&delegation.target.invocation_id)
    .bind(&delegation.target.session_id)
    .bind(&delegation.target.run_id)
    .bind(&delegation.target.turn_id)
    .fetch_one(&mut *tx)
    .await?;
    if occupied {
        return Err(invalid("WorkHub target execution identity already exists"));
    }
    let (revision, fingerprint): (i64, String) =
        sqlx::query_as("SELECT revision, fingerprint FROM session_control WHERE id = ?")
            .bind(&delegation.target.session_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StoreError::SessionNotFound)?;
    if delegation.kind == maka_runtime::workhub::DelegationKind::Created
        && fingerprint != format!("workhub.create:{}", delegation.request_fingerprint)
    {
        return Err(invalid("WorkHub target was not created by this action"));
    }
    if u64::try_from(revision).ok() != Some(delegation.target_revision) {
        return Err(StoreError::RevisionConflict {
            expected: delegation.target_revision.to_string(),
            actual: revision.to_string(),
        });
    }
    crate::context::safety::require_safe(tx, &delegation.target.session_id, None).await?;
    if crate::shell_runs::unsettled(tx, &delegation.target.session_id).await? {
        return Err(StoreError::SessionBusy);
    }
    let admitted_at = event
        .recorded_at
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| invalid("invalid WorkHub admission time"))?
        .as_millis()
        .try_into()
        .map_err(|_| invalid("invalid WorkHub admission time"))?;
    let message = delegation.message(&content).map_err(invalid)?;
    for (source, destination) in content
        .attachments
        .iter()
        .flatten()
        .zip(message.message.content.attachments.iter().flatten())
    {
        crate::artifacts::copy_in_transaction(
            tx,
            source,
            destination,
            &delegation.target.turn_id,
            admitted_at,
        )
        .await?;
    }
    let admission = PendingMessageAdmission {
        invocation: delegation.target.clone(),
        steering_invocation: None,
        source: message,
        required_tools: Default::default(),
        admitted_at,
    };
    crate::message_admissions::insert::insert(
        tx,
        &admission,
        crate::message_admissions::insert::Owner::Unsealed,
    )
    .await?;
    Ok(())
}

fn invalid(reason: &str) -> StoreError {
    StoreError::InvalidTransition(reason.into())
}
