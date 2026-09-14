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

use crate::{EventLog, StoreError, sequence_number, sessions, turns};
use futures_util::TryStreamExt;
use maka_runtime::event::{RuntimeEvent, StoredEvent};
use maka_runtime::interaction::InteractionRecord;
use serde::de::DeserializeOwned;
use sqlx::{Connection, Row};

mod delivery;
mod streams;
pub use delivery::{StoreStreamEvent, StreamEventPage, StreamFact, ToolSettlement};
pub use streams::AssistantStreamSeed;
pub(crate) use streams::register_function;

/// Metadata, latest root Turn and catch-up cursor from the same SQL snapshot.
pub struct SessionObservation<T> {
    pub session: sessions::SessionRecord<T>,
    pub root_turn: Option<turns::TurnBoundary>,
    pub through_sequence: u64,
    pub active_streams: Vec<AssistantStreamSeed>,
    pub pending_interactions: Vec<InteractionRecord>,
    pub message_queue: crate::message_queue::MessageQueue,
}

/// Projection-only read: no stream offset aggregation on token wakeups.
pub struct SessionProjection<T> {
    pub session: sessions::SessionRecord<T>,
    pub root_turn: Option<turns::TurnBoundary>,
    pub through_sequence: u64,
    pub pending_interactions: Vec<InteractionRecord>,
    pub message_queue: crate::message_queue::MessageQueue,
}

/// Observation pages are ordered delivery material, not model replay proofs.
pub struct EventPage {
    pub events: Vec<StoredEvent>,
    pub through_sequence: u64,
    /// Continue strictly after this sequence; None means the fence is exhausted.
    pub next_after: Option<u64>,
}

impl EventLog {
    pub async fn session_projection<T: DeserializeOwned + Send + 'static>(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionProjection<T>>, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session_id)?;
        let session_id = session_id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut transaction = connection.begin().await?;
                    let result = read_projection(&mut transaction, &session_id).await?;
                    transaction.commit().await?;
                    Ok(result)
                })
            })
            .await
    }

    pub async fn observe_session<T: DeserializeOwned + Send + 'static>(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionObservation<T>>, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session_id)?;
        let session_id = session_id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut transaction = connection.begin().await?;
                    let Some(SessionProjection {
                        session,
                        root_turn,
                        through_sequence,
                        pending_interactions,
                        message_queue,
                    }) = read_projection(&mut transaction, &session_id).await?
                    else {
                        return Ok(None);
                    };
                    let active_streams =
                        streams::read(&mut transaction, root_turn.as_ref()).await?;
                    transaction.commit().await?;
                    Ok(Some(SessionObservation {
                        session,
                        root_turn,
                        through_sequence,
                        active_streams,
                        pending_interactions,
                        message_queue,
                    }))
                })
            })
            .await
    }

    /// Subscribe to commit wakeups before taking the initial fence. Catch-up
    /// consumes only (after, through]; later commits cannot change this page.
    pub async fn session_events(
        &self,
        session_id: &str,
        after: u64,
        through: u64,
        max_events: usize,
        max_bytes: usize,
    ) -> Result<EventPage, StoreError> {
        self.validate_root()?;
        sessions::validate_id(session_id)?;
        if after > through || max_events == 0 || max_events > 512 || max_bytes == 0 {
            return Err(invalid("invalid observation page bounds"));
        }
        let after_sql = i64::try_from(after).map_err(|_| invalid("invalid observation cursor"))?;
        let through_sql =
            i64::try_from(through).map_err(|_| invalid("invalid observation fence"))?;
        let session_id = session_id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut transaction = connection.begin().await?;
                    if through > high_water(&mut transaction).await? {
                        return Err(invalid("observation fence is beyond committed history"));
                    }
                    let mut events: Vec<StoredEvent> = Vec::new();
                    let mut bytes = 0usize;
                    let mut next_after = None;
                    {
                        let mut rows = sqlx::query(
                            "SELECT sequence, length(CAST(event_json AS BLOB)), event_json FROM runtime_events
                             WHERE json_extract(event_json, '$.invocation.session_id') = ?1
                             AND sequence > ?2 AND sequence <= ?3 ORDER BY sequence LIMIT ?4",
                        )
                        .bind(&session_id)
                        .bind(after_sql)
                        .bind(through_sql)
                        .bind(max_events as i64 + 1)
                        .fetch(&mut *transaction);
                        while let Some(row) = rows.try_next().await? {
                            let size = usize::try_from(row.try_get::<i64, _>(1)?)
                                .map_err(|_| StoreError::PrefixTooLarge)?;
                            if events.len() == max_events || size > max_bytes.saturating_sub(bytes)
                            {
                                let last = events.last().ok_or(StoreError::PrefixTooLarge)?;
                                next_after = Some(last.sequence);
                                break;
                            }
                            let sequence = sequence_number(row.try_get(0)?)?;
                            let event: RuntimeEvent =
                                serde_json::from_str(row.try_get::<&str, _>(2)?)?;
                            if event.invocation.session_id != session_id {
                                return Err(invalid("stored observation scope changed"));
                            }
                            bytes += size;
                            events.push(StoredEvent { sequence, event });
                        }
                    }
                    transaction.commit().await?;
                    Ok(EventPage {
                        events,
                        through_sequence: through,
                        next_after,
                    })
                })
            })
            .await
    }
}

async fn read_projection<T: DeserializeOwned + Send>(
    connection: &mut sqlx::SqliteConnection,
    session_id: &str,
) -> Result<Option<SessionProjection<T>>, StoreError> {
    let Some(session) = sessions::read(connection, session_id).await? else {
        return Ok(None);
    };
    Ok(Some(SessionProjection {
        session,
        root_turn: turns::read(connection, session_id, None).await?,
        through_sequence: high_water(connection).await?,
        pending_interactions: crate::interactions::pending(connection, session_id).await?,
        message_queue: crate::message_queue::read(connection, session_id).await?,
    }))
}

async fn high_water(connection: &mut sqlx::SqliteConnection) -> Result<u64, StoreError> {
    sequence_number(
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM runtime_events")
            .fetch_one(connection)
            .await?,
    )
}
fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
