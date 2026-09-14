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

use crate::{EventLog, StoreError, sessions};
use maka_runtime::event::{Fact, Invocation, InvocationOutcome, RuntimeEvent};
use maka_runtime::input::InvocationInput;
use sqlx::Connection;

/// A bounded projection of canonical boundaries, never a model-history prefix.
pub struct TurnBoundary {
    pub invocation: Invocation,
    pub input: InvocationInput,
    pub state: InvocationState,
}
pub enum InvocationState {
    Admitted,
    Running,
    WaitingForUser,
    Ended {
        event_id: String,
        outcome: InvocationOutcome,
    },
}

impl EventLog {
    /// Exact physical Run, including an earlier Run of the same logical Turn.
    pub async fn run_boundary(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> Result<Option<TurnBoundary>, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session_id)?;
        sessions::validate_id(run_id)?;
        let (session_id, run_id) = (session_id.to_owned(), run_id.to_owned());
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let Some(invocation) = run_invocation(&mut tx, &session_id, &run_id).await? else {
                        return Ok(None);
                    };
                    let opening: String = sqlx::query_scalar(
                        "SELECT event_json FROM runtime_events WHERE invocation_id = ? AND kind = 'invocation_opened'",
                    ).bind(invocation).fetch_one(&mut *tx).await?;
                    let result = project(&mut tx, serde_json::from_str(&opening)?).await?;
                    tx.commit().await?;
                    Ok(Some(result))
                })
            })
            .await
    }

    /// At most two event bodies are decoded, irrespective of streamed history.
    /// Reading control/status must remain possible after context budgets fill.
    pub async fn turn_boundary(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<TurnBoundary>, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session_id)?;
        sessions::validate_id(turn_id)?;
        let session_id = session_id.to_owned();
        let turn_id = turn_id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut transaction = connection.begin().await?;
                    let result = read(&mut transaction, &session_id, Some(&turn_id)).await?;
                    transaction.commit().await?;
                    Ok(result)
                })
            })
            .await
    }
}

/// Caller holds one read transaction when combining with metadata or a cursor.
pub(crate) async fn read(
    connection: &mut sqlx::SqliteConnection,
    session_id: &str,
    turn_id: Option<&str>,
) -> Result<Option<TurnBoundary>, StoreError> {
    let opening: Option<String> = sqlx::query_scalar(
        "SELECT event_json FROM runtime_events
             WHERE kind = 'invocation_opened'
             AND json_extract(event_json, '$.invocation.session_id') = ?1
             AND (?2 IS NULL OR json_extract(event_json, '$.invocation.turn_id') = ?2)
             ORDER BY sequence DESC LIMIT 1",
    )
    .bind(session_id)
    .bind(turn_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(opening) = opening else {
        return Ok(None);
    };
    Ok(Some(
        project(connection, serde_json::from_str(&opening)?).await?,
    ))
}

/// Fail closed on ambiguous legacy/synthetic identities; never choose a newer Run.
pub(crate) async fn run_invocation(
    connection: &mut sqlx::SqliteConnection,
    session: &str,
    run: &str,
) -> Result<Option<String>, StoreError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT invocation_id FROM runtime_events WHERE kind = 'invocation_opened'
         AND json_extract(event_json, '$.invocation.session_id') = ?
         AND json_extract(event_json, '$.invocation.run_id') = ? LIMIT 2",
    )
    .bind(session)
    .bind(run)
    .fetch_all(connection)
    .await?;
    if ids.len() > 1 {
        return Err(StoreError::InvalidTransition(
            "Run names multiple invocations".into(),
        ));
    }
    Ok(ids.into_iter().next())
}

async fn project(
    connection: &mut sqlx::SqliteConnection,
    opening: RuntimeEvent,
) -> Result<TurnBoundary, StoreError> {
    let Fact::InvocationOpened { input, .. } = opening.fact else {
        return Err(StoreError::InvalidTransition(
            "invalid stored invocation opening".into(),
        ));
    };
    let terminal: Option<String> = sqlx::query_scalar(
            "SELECT event_json FROM runtime_events WHERE invocation_id = ? AND kind = 'invocation_ended'",
        ).bind(&opening.invocation.invocation_id).fetch_optional(&mut *connection).await?;
    let state = if let Some(terminal) = terminal {
        let terminal: RuntimeEvent = serde_json::from_str(&terminal)?;
        let Fact::InvocationEnded { outcome } = terminal.fact else {
            return Err(StoreError::InvalidTransition(
                "invalid stored invocation terminal".into(),
            ));
        };
        if terminal.invocation != opening.invocation {
            return Err(StoreError::InvalidTransition(
                "stored invocation identity changed".into(),
            ));
        }
        InvocationState::Ended {
            event_id: terminal.id,
            outcome,
        }
    } else if crate::interactions::pending(connection, &opening.invocation.session_id)
        .await?
        .iter()
        .any(|request| {
            request.run_id == opening.invocation.run_id
                && request.turn_id == opening.invocation.turn_id
        })
    {
        InvocationState::WaitingForUser
    } else {
        let requested: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id = ? AND kind = 'model_requested')",
            ).bind(&opening.invocation.invocation_id).fetch_one(&mut *connection).await?;
        if requested {
            InvocationState::Running
        } else {
            InvocationState::Admitted
        }
    };
    Ok(TurnBoundary {
        invocation: opening.invocation,
        input,
        state,
    })
}
