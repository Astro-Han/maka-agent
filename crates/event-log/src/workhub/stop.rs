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
use maka_runtime::{
    event::Invocation,
    workhub::{COORDINATION_SESSION_ID, StopOutcome},
};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, SqliteConnection};

mod admit;
mod subject;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopRequest {
    pub action_id: String,
    pub request_fingerprint: String,
    pub source: Invocation,
    pub target_session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopResolution {
    pub outcome: StopOutcome,
    pub target_turn_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopIntent {
    pub request: StopRequest,
    pub delegation_action_id: String,
    /// Frozen before cancellation. A retry must never follow a later continuation.
    pub owner: Option<Invocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopRecord {
    pub intent: StopIntent,
    pub resolution: Option<StopResolution>,
}

impl StopRequest {
    fn validate(&self) -> Result<(), StoreError> {
        for id in [
            &self.action_id,
            &self.source.session_id,
            &self.source.turn_id,
            &self.source.run_id,
            &self.source.invocation_id,
            &self.target_session_id,
        ] {
            crate::sessions::validate_id(id)?;
        }
        if self.source.session_id != COORDINATION_SESSION_ID
            || self.target_session_id == COORDINATION_SESSION_ID
            || !maka_runtime::archive::valid_projection_digest(&self.request_fingerprint)
        {
            return Err(invalid("Invalid WorkHub stop authority"));
        }
        Ok(())
    }
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
        request.validate()?;
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
                    let record = admit::apply(&mut tx, request).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_modify(|_| {});
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
                    let record = resolve(&mut tx, &action).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_modify(|_| {});
                    Ok(record)
                })
            })
            .await
    }

    /// Startup only, after abandoned Run recovery and before pending work starts.
    pub async fn recover_workhub_stops(&self) -> Result<usize, StoreError> {
        self.validate_root()?;
        self.connection.run(move |connection| Box::pin(async move {
            let mut recovered = 0;
            loop {
                let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                let actions: Vec<String> = sqlx::query_scalar(
                    "SELECT action_id FROM workhub_stops WHERE resolution_json IS NULL ORDER BY action_id LIMIT 64"
                ).fetch_all(&mut *tx).await?;
                if actions.is_empty() { return Ok(recovered); }
                for action in actions {
                    resolve(&mut tx, &action).await?;
                    recovered += 1;
                }
                tx.commit().await.map_err(StoreError::CommitUnknown)?;
            }
        })).await
    }
}

async fn read(tx: &mut SqliteConnection, action: &str) -> Result<Option<StopRecord>, StoreError> {
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
    intent.request.validate()?;
    if intent.request.action_id != action {
        return Err(invalid("WorkHub stop index changed"));
    }
    crate::sessions::validate_id(&intent.delegation_action_id)?;
    if let Some(owner) = &intent.owner {
        if owner.session_id != intent.request.target_session_id {
            return Err(invalid("WorkHub stop owner belongs to another Session"));
        }
        for id in [&owner.turn_id, &owner.run_id, &owner.invocation_id] {
            crate::sessions::validate_id(id)?;
        }
    }
    let resolution: Option<StopResolution> = resolution
        .map(|json| serde_json::from_str(&json))
        .transpose()?;
    if resolution.is_none() && intent.owner.is_none() {
        return Err(invalid("Unresolved WorkHub stop has no owner"));
    }
    if let Some(turn) = resolution
        .as_ref()
        .and_then(|result| result.target_turn_id.as_ref())
    {
        crate::sessions::validate_id(turn)?;
    }
    Ok(Some(StopRecord { intent, resolution }))
}

async fn resolve(tx: &mut SqliteConnection, action: &str) -> Result<StopRecord, StoreError> {
    let mut record = read(tx, action)
        .await?
        .ok_or_else(|| invalid("WorkHub stop intent is missing"))?;
    if record.resolution.is_some() {
        return Ok(record);
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
    sqlx::query("UPDATE workhub_stops SET resolution_json = ? WHERE action_id = ? AND resolution_json IS NULL")
        .bind(serde_json::to_string(&resolution)?).bind(action).execute(tx).await?;
    record.resolution = Some(resolution);
    Ok(record)
}

fn invalid(reason: &str) -> StoreError {
    StoreError::InvalidTransition(reason.into())
}
