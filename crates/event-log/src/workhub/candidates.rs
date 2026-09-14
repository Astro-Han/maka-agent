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

use crate::{
    EventLog, StoreError,
    sessions::{SessionExecutionState, SessionRecord},
};
use maka_runtime::event::Invocation;
use maka_runtime::workhub::ActionId;
use serde::de::DeserializeOwned;
use sqlx::Connection;

pub struct Candidate<T> {
    pub session: SessionRecord<T>,
    pub latest_delegation_action_id: Option<ActionId>,
}

impl EventLog {
    /// Revalidate an offered target independently of the current display window.
    pub async fn workhub_candidate<T, F>(
        &self,
        id: &str,
        eligible: F,
    ) -> Result<Option<SessionRecord<T>>, StoreError>
    where
        T: DeserializeOwned + Send + Sync + 'static,
        F: Fn(&SessionRecord<T>) -> bool + Send + 'static,
    {
        self.validate_root()?;
        crate::sessions::validate_id(id)?;
        let id = id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let Some(record) = crate::sessions::read(&mut tx, &id).await? else {
                        return Ok(None);
                    };
                    if !eligible(&record) || !available(&mut tx, &record).await? {
                        return Ok(None);
                    }
                    Ok(Some(record))
                })
            })
            .await
    }

    /// One read snapshot, bounded resident records, no independent candidate authority.
    /// Discovery includes waiting and blocked work; action admission checks write safety.
    pub async fn workhub_candidates<T, F>(
        &self,
        eligible: F,
    ) -> Result<Vec<Candidate<T>>, StoreError>
    where
        T: DeserializeOwned + Send + Sync + 'static,
        F: Fn(&SessionRecord<T>) -> bool + Send + 'static,
    {
        self.validate_root()?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let mut cursor = String::new();
                    let mut candidates = Vec::new();
                    loop {
                        let ids: Vec<String> = sqlx::query_scalar(
                            "SELECT id FROM session_control WHERE archived = 0 AND id > ?
                     ORDER BY id LIMIT 32",
                        )
                        .bind(&cursor)
                        .fetch_all(&mut *tx)
                        .await?;
                        if ids.is_empty() {
                            break;
                        }
                        for id in &ids {
                            let record = crate::sessions::read(&mut tx, id)
                                .await?
                                .ok_or(StoreError::SessionNotFound)?;
                            if !eligible(&record) {
                                continue;
                            }
                            candidates.push(record);
                            candidates.sort_by(|a, b| {
                                activity_at(b)
                                    .cmp(&activity_at(a))
                                    .then_with(|| a.id.cmp(&b.id))
                            });
                            candidates.truncate(32);
                        }
                        cursor = ids.last().unwrap().clone();
                    }
                    let mut result = Vec::with_capacity(candidates.len());
                    for session in candidates {
                        let latest_delegation_action_id: Option<String> = sqlx::query_scalar(
                            "SELECT json_extract(assignment.event_json, '$.fact.delegation.action_id')
                             FROM event_log assignment
                             WHERE assignment.kind = 'workhub_delegated'
                               AND json_extract(assignment.event_json, '$.fact.delegation.target.session_id') = ?
                               AND NOT EXISTS (SELECT 1 FROM workhub_corrections correction
                                   WHERE correction.replaces_action_id = json_extract(assignment.event_json, '$.fact.delegation.action_id')
                                     AND correction.resolution_kind IS NOT NULL)
                               AND NOT EXISTS (SELECT 1 FROM workhub_stops stop
                                   WHERE stop.delegation_action_id = json_extract(assignment.event_json, '$.fact.delegation.action_id')
                                     AND json_extract(stop.resolution_json, '$.outcome') != 'not_owned')
                             ORDER BY assignment.sequence DESC LIMIT 1",
                        ).bind(&session.id).fetch_optional(&mut *tx).await?;
                        let latest_delegation_action_id = latest_delegation_action_id
                            .map(ActionId::new).transpose().map_err(super::invalid)?;
                        result.push(Candidate { session, latest_delegation_action_id });
                    }
                    tx.commit().await?;
                    Ok(result)
                })
            })
            .await
    }
}

async fn available<T>(
    tx: &mut sqlx::SqliteConnection,
    record: &SessionRecord<T>,
) -> Result<bool, StoreError> {
    if record.archived {
        return Ok(false);
    }
    let live = record
        .execution
        .as_ref()
        .is_some_and(|execution| matches!(execution.state, SessionExecutionState::Live { .. }));
    let owner = if live {
        let json: String = sqlx::query_scalar(
            "SELECT json_extract(event_json, '$.invocation') FROM runtime_events
             WHERE kind = 'invocation_opened' AND json_extract(event_json, '$.invocation.session_id') = ?
             ORDER BY sequence DESC LIMIT 1"
        ).bind(&record.id).fetch_one(&mut *tx).await?;
        Some(serde_json::from_str::<Invocation>(&json)?)
    } else {
        None
    };
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_admissions WHERE session_id = ?)")
            .bind(&record.id)
            .fetch_one(&mut *tx)
            .await?;
    if pending && owner.is_none() {
        return Ok(false);
    }
    match require_available(tx, &record.id, owner.as_ref()).await {
        Ok(()) => Ok(true),
        Err(StoreError::InvalidTransition(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Target admission guard. Current effects stay with their live owner;
/// a sealed owner cannot hide unknown effects behind a late steering admission.
pub(super) async fn require_available(
    tx: &mut sqlx::SqliteConnection,
    session: &str,
    owner: Option<&Invocation>,
) -> Result<(), StoreError> {
    if !crate::interactions::pending(tx, session).await?.is_empty() {
        return Err(super::invalid("WorkHub target is waiting for user input"));
    }
    let current = if let Some(owner) = owner {
        let live: Option<bool> = sqlx::query_scalar(
            "SELECT NOT EXISTS(SELECT 1 FROM runtime_events t WHERE t.invocation_id = o.invocation_id
                AND t.kind = 'invocation_ended') FROM runtime_events o WHERE o.invocation_id = ?
             AND o.kind = 'invocation_opened' AND json_extract(o.event_json, '$.fact.input.kind') IN ('message', 'continuation')
             AND json_extract(o.event_json, '$.invocation.session_id') = ?"
        ).bind(&owner.invocation_id).bind(session).fetch_optional(&mut *tx).await?;
        let live = live.ok_or_else(|| {
            super::invalid("WorkHub steering requires a message or continuation owner")
        })?;
        live.then_some(owner)
    } else {
        None
    };
    crate::context::safety::require_safe(
        tx,
        session,
        current.map(|owner| owner.invocation_id.as_str()),
    )
    .await?;
    if crate::shell_runs::unsettled_outside(tx, session, current.map(|owner| owner.run_id.as_str()))
        .await?
    {
        return Err(super::invalid("WorkHub target has unsettled shell work"));
    }
    Ok(())
}

pub fn activity_at<T>(record: &SessionRecord<T>) -> u64 {
    record
        .execution
        .as_ref()
        .map_or(record.created_at, |execution| {
            execution.last_message.as_ref().map_or_else(
                || match execution.state {
                    SessionExecutionState::Live { recorded_at }
                    | SessionExecutionState::Ended { recorded_at, .. } => recorded_at,
                },
                |message| message.recorded_at,
            )
        })
}
