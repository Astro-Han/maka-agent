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
use futures_util::TryStreamExt;
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

    /// Rank lightweight rows in one read snapshot, then hydrate only the visible
    /// candidates. No per-session projection scan or independent candidate authority.
    /// Discovery includes waiting and blocked work; action admission checks write safety.
    pub async fn workhub_candidates<T, F>(
        &self,
        eligible: F,
    ) -> Result<Vec<Candidate<T>>, StoreError>
    where
        T: DeserializeOwned + Send + Sync + 'static,
        F: Fn(&str, &T) -> bool + Send + 'static,
    {
        self.validate_root()?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let mut rows = sqlx::query_as::<_, (String, String)>(
                        "SELECT session.id, session.configuration
                         FROM session_control session
                         LEFT JOIN event_log opening ON opening.sequence = (
                             SELECT sequence FROM event_log INDEXED BY catalog_message_facts
                             WHERE invocation_id IS NOT NULL
                               AND kind IN ('invocation_opened', 'model_completed')
                               AND kind = 'invocation_opened'
                               AND json_extract(event_json, '$.invocation.session_id') = session.id
                             ORDER BY sequence DESC LIMIT 1)
                         WHERE session.archived = 0
                         ORDER BY CASE WHEN opening.sequence IS NULL THEN session.created_at
                           ELSE COALESCE(
                             (SELECT message_at FROM catalog_messages INDEXED BY catalog_latest_message
                              WHERE session_id = session.id
                              ORDER BY message_at DESC, sequence DESC, ordinal DESC LIMIT 1),
                             (SELECT catalog_time(json_extract(event_json, '$.recorded_at'))
                              FROM event_log INDEXED BY invocation_boundary
                              WHERE invocation_id = opening.invocation_id
                                AND kind IN ('invocation_opened', 'invocation_ended')
                                AND kind = 'invocation_ended' ORDER BY sequence DESC LIMIT 1),
                             catalog_time(json_extract(opening.event_json, '$.recorded_at')))
                           END DESC, session.id ASC",
                    ).fetch(&mut *tx);
                    let mut ids = Vec::with_capacity(32);
                    while let Some((id, configuration)) = rows.try_next().await? {
                        if eligible(&id, &serde_json::from_str::<T>(&configuration)?) {
                            ids.push(id);
                            if ids.len() == 32 { break; }
                        }
                    }
                    drop(rows);
                    let mut result = Vec::with_capacity(ids.len());
                    for id in ids {
                        let session = crate::sessions::read(&mut tx, &id)
                            .await?.ok_or(StoreError::SessionNotFound)?;
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
             AND o.kind = 'invocation_opened' AND json_extract(o.event_json, '$.fact.input.kind') IN ('message', 'continuation', 'handoff')
             AND json_extract(o.event_json, '$.invocation.session_id') = ?"
        ).bind(&owner.invocation_id).bind(session).fetch_optional(&mut *tx).await?;
        let live =
            live.ok_or_else(|| super::invalid("WorkHub steering requires an inline model owner"))?;
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
