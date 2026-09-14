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

use crate::{EventLog, StoreError};
pub use maka_runtime::workhub::{StopIntent, StopRequest, StopResolution};
use maka_runtime::{
    session_event::{SessionEvent, SessionFact},
    workhub::StopOutcome,
};
use sqlx::{Connection, SqliteConnection};

mod admit;
mod subject;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopRecord {
    pub intent: StopIntent,
    pub resolution: Option<StopResolution>,
}

impl EventLog {
    pub async fn workhub_stop(&self, action: &str) -> Result<Option<StopRecord>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(action)?;
        let action = action.to_owned();
        self.connection
            .run(move |tx| Box::pin(async move { read(tx, &action).await }))
            .await
    }

    /// Intent, exact pending cancellation and its resolution share one commit.
    /// A Run cancellation happens only after the returned unresolved intent.
    pub async fn request_workhub_stop(
        &self,
        request: StopRequest,
    ) -> Result<StopRecord, StoreError> {
        self.validate_root()?;
        request.validate().map_err(invalid)?;
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    if let Some(previous) = read(&mut tx, &request.action_id).await? {
                        if previous.intent.request != request {
                            return Err(invalid("WorkHub stop identity changed"));
                        }
                        return Ok(previous);
                    }
                    let (record, sequence) = admit::apply(&mut tx, request).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_replace(sequence);
                    Ok(record)
                })
            })
            .await
    }

    /// Finalize only the frozen owner; this never dispatches work or cancellation.
    pub async fn resolve_workhub_stop(&self, action: &str) -> Result<StopRecord, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(action)?;
        let action = action.to_owned();
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let (record, sequence) = resolve(&mut tx, &action).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    if let Some(sequence) = sequence {
                        commits.send_replace(sequence);
                    }
                    Ok(record)
                })
            })
            .await
    }

    /// Startup only, after abandoned Run recovery and before pending work starts.
    pub async fn recover_workhub_stops(&self) -> Result<usize, StoreError> {
        self.validate_root()?;
        let commits = self.commits.clone();
        self.connection.run(move |connection| Box::pin(async move {
            let mut recovered = 0;
            loop {
                let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                let actions: Vec<String> = sqlx::query_scalar(
                    "SELECT action_id FROM workhub_stops WHERE resolution_json IS NULL ORDER BY action_id LIMIT 64"
                ).fetch_all(&mut *tx).await?;
                if actions.is_empty() { return Ok(recovered); }
                let mut last = None;
                for action in actions {
                    let (_, sequence) = resolve(&mut tx, &action).await?;
                    last = sequence.or(last);
                    recovered += 1;
                }
                tx.commit().await.map_err(StoreError::CommitUnknown)?;
                if let Some(sequence) = last {
                    commits.send_replace(sequence);
                }
            }
        })).await
    }
}

pub(crate) async fn read(
    tx: &mut SqliteConnection,
    action: &str,
) -> Result<Option<StopRecord>, StoreError> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT CASE WHEN length(CAST(record_json AS BLOB)) <= 16384 THEN record_json END,
         resolution_json FROM workhub_stops WHERE action_id = ?",
    )
    .bind(action)
    .fetch_optional(tx)
    .await?;
    let Some((record, resolution)) = row else {
        return Ok(None);
    };
    let intent: StopIntent = serde_json::from_str(&record.ok_or(StoreError::PrefixTooLarge)?)?;
    intent.validate().map_err(invalid)?;
    if intent.request.action_id != action {
        return Err(invalid("WorkHub stop index changed"));
    }
    let resolution: Option<StopResolution> = resolution
        .map(|json| serde_json::from_str(&json))
        .transpose()?;
    if resolution.is_none() && intent.owner.is_none() {
        return Err(invalid("Unresolved WorkHub stop has no owner"));
    }
    if let Some(resolution) = &resolution {
        resolution.validate().map_err(invalid)?;
    }
    Ok(Some(StopRecord { intent, resolution }))
}

async fn resolve(
    tx: &mut SqliteConnection,
    action: &str,
) -> Result<(StopRecord, Option<u64>), StoreError> {
    let mut record = read(tx, action)
        .await?
        .ok_or_else(|| invalid("WorkHub stop intent is missing"))?;
    if record.resolution.is_some() {
        return Ok((record, None));
    }
    let owner = record
        .intent
        .owner
        .as_ref()
        .ok_or_else(|| invalid("WorkHub stop owner is missing"))?;
    let terminal: Option<Option<String>> = sqlx::query_scalar(
        "SELECT CASE WHEN length(CAST(event_json AS BLOB)) <= 1048576 THEN event_json END
         FROM runtime_events WHERE invocation_id = ? AND kind = 'invocation_ended'",
    )
    .bind(&owner.invocation_id)
    .fetch_optional(&mut *tx)
    .await?;
    let terminal: maka_runtime::event::RuntimeEvent = serde_json::from_str(
        &terminal
            .ok_or(StoreError::SessionBusy)?
            .ok_or(StoreError::PrefixTooLarge)?,
    )?;
    if terminal.invocation != *owner {
        return Err(invalid("WorkHub stop terminal owner changed"));
    }
    let maka_runtime::event::Fact::InvocationEnded { outcome } = terminal.fact else {
        return Err(invalid("WorkHub stop owner has no terminal"));
    };
    let resolution = StopResolution {
        outcome: if matches!(
            outcome,
            maka_runtime::event::InvocationOutcome::Cancelled { source }
                if source == maka_runtime::workhub::stop_abort_source(action)
        ) {
            StopOutcome::StopDelivered
        } else {
            StopOutcome::AlreadyTerminal
        },
        target_turn_id: Some(owner.turn_id.clone()),
    };
    let sequence = super::control::append(
        tx,
        &SessionEvent::workhub(
            record.intent.request.source.turn_id.clone(),
            SessionFact::WorkhubStopResolved {
                action_id: action.into(),
                resolution: resolution.clone(),
            },
        ),
    )
    .await?;
    record.resolution = Some(resolution);
    Ok((record, Some(sequence)))
}

fn invalid(reason: &str) -> StoreError {
    StoreError::InvalidTransition(reason.into())
}
