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

use super::invalid;
use crate::{EventLog, StoreError, sequence_number};
use maka_runtime::workhub::ActionId;
use maka_runtime::{event::Invocation, workhub::Delegation};
use sqlx::SqliteConnection;
use std::time::SystemTime;

/// A canonical assignment independent of whether its owner is a Run or Session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub sequence: u64,
    pub id: String,
    pub recorded_at: SystemTime,
    pub coordinator: Invocation,
    pub delegation: Box<Delegation>,
}

impl EventLog {
    pub async fn workhub_assignment(
        &self,
        action: &ActionId,
    ) -> Result<Option<Assignment>, StoreError> {
        self.validate_root()?;
        let action = action.to_owned();
        self.connection
            .run(move |tx| Box::pin(async move { read(tx, &action).await }))
            .await
    }
}

pub(crate) async fn read(
    tx: &mut SqliteConnection,
    action: &ActionId,
) -> Result<Option<Assignment>, StoreError> {
    let row: Option<(i64, Option<String>, String)> = sqlx::query_as(
        "SELECT sequence, CASE WHEN length(CAST(event_json AS BLOB)) <= 1048576 THEN event_json END,
         coordinator_json FROM workhub_assignments
         WHERE json_extract(event_json, '$.fact.delegation.action_id') = ?"
    ).bind(action.as_str()).fetch_optional(tx).await?;
    row.map(|(sequence, json, coordinator)| {
        #[derive(serde::Deserialize)]
        struct Envelope {
            id: String,
            recorded_at: SystemTime,
            fact: Assigned,
        }
        #[derive(serde::Deserialize)]
        struct Assigned {
            delegation: Box<Delegation>,
        }
        let envelope: Envelope = serde_json::from_str(&json.ok_or(StoreError::PrefixTooLarge)?)?;
        let coordinator = serde_json::from_str(&coordinator)?;
        envelope
            .fact
            .delegation
            .validate(&coordinator)
            .map_err(invalid)?;
        if &envelope.fact.delegation.action_id != action {
            return Err(invalid("WorkHub assignment index changed"));
        }
        Ok(Assignment {
            sequence: sequence_number(sequence)?,
            id: envelope.id,
            recorded_at: envelope.recorded_at,
            coordinator,
            delegation: envelope.fact.delegation,
        })
    })
    .transpose()
}

/// Correction claims retire the association even if their replacement aborts.
pub(super) async fn require_unclaimed(
    tx: &mut SqliteConnection,
    action: &ActionId,
) -> Result<(), StoreError> {
    let claimed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workhub_corrections WHERE replaces_action_id = ?1
         UNION ALL SELECT 1 FROM workhub_stops WHERE delegation_action_id = ?1
          AND (resolution_json IS NULL OR json_extract(resolution_json, '$.outcome') != 'not_owned'))"
    ).bind(action.as_str()).fetch_one(tx).await?;
    if claimed {
        return Err(invalid("WorkHub delegation already has a retirement claim"));
    }
    Ok(())
}
