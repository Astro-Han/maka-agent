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

//! Disposable identity lookup over immutable opening and steering facts.
use crate::{EventLog, StoreError, sequence_number};
use maka_runtime::{
    event::{Fact, InvocationInput, RuntimeEvent, StoredEvent},
    message::RootSourceMessage,
};
use sqlx::{Connection, SqliteConnection};

pub(crate) fn roots(event: &RuntimeEvent) -> &[RootSourceMessage] {
    match &event.fact {
        Fact::InvocationOpened {
            input: InvocationInput::Message {
                source_messages, ..
            },
            ..
        } => source_messages,
        _ => &[],
    }
}
pub(crate) fn identities(event: &RuntimeEvent) -> impl Iterator<Item = &str> {
    roots(event)
        .iter()
        .map(|source| source.message.message_id.as_str())
        .chain(match &event.fact {
            Fact::MessageSteered { message, .. } => Some(message.message_id.as_str()),
            _ => None,
        })
}
pub(crate) async fn initialize(connection: &mut SqliteConnection) -> Result<(), StoreError> {
    let populated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_sources)")
        .fetch_one(&mut *connection)
        .await?;
    if populated {
        return Ok(());
    }
    let mut tx = connection.begin().await?;
    sqlx::raw_sql(
        "INSERT INTO message_sources
         SELECT json_extract(event_json, '$.invocation.session_id'),
            json_extract(event_json, '$.fact.message.message_id'), event_id
         FROM runtime_events WHERE kind = 'message_steered';
         INSERT INTO message_sources
         SELECT json_extract(e.event_json, '$.invocation.session_id'),
            json_extract(s.value, '$.message_id'), e.event_id
         FROM runtime_events e, json_each(e.event_json, '$.fact.input.source_messages') s
         WHERE e.kind = 'invocation_opened' AND json_extract(e.event_json, '$.fact.input.kind') = 'message';"
    ).execute(&mut *tx).await?;
    tx.commit().await.map_err(StoreError::CommitUnknown)?;
    Ok(())
}
pub(crate) async fn insert(
    connection: &mut SqliteConnection,
    event: &RuntimeEvent,
) -> Result<(), StoreError> {
    for id in identities(event) {
        sqlx::query("INSERT INTO message_sources VALUES (?, ?, ?)")
            .bind(&event.invocation.session_id)
            .bind(id)
            .bind(&event.id)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct RootMessageProof {
    opening: StoredEvent,
    index: usize,
}
impl RootMessageProof {
    pub fn opening(&self) -> &StoredEvent {
        &self.opening
    }
    pub fn source(&self) -> &RootSourceMessage {
        &roots(&self.opening.event)[self.index]
    }
}

impl EventLog {
    /// Resolve an original source identity without inferring ownership from UI rows.
    pub async fn root_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<RootMessageProof>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session_id)?;
        crate::sessions::validate_id(message_id)?;
        let session = session_id.to_owned();
        let message = message_id.to_owned();
        self.connection.run(move |connection| Box::pin(async move {
            let row: Option<(i64, Option<String>)> = sqlx::query_as(
                "SELECT e.sequence, CASE WHEN length(CAST(e.event_json AS BLOB)) <= 1048576 THEN e.event_json END
                 FROM message_sources s JOIN runtime_events e ON e.event_id = s.event_id
                 WHERE s.session_id = ? AND s.message_id = ? AND e.kind = 'invocation_opened'"
            ).bind(&session).bind(&message).fetch_optional(connection).await?;
            let Some((sequence, json)) = row else { return Ok(None); };
            let event: RuntimeEvent = serde_json::from_str(&json.ok_or(StoreError::PrefixTooLarge)?)?;
            let Fact::InvocationOpened { input: InvocationInput::Message { content, source_messages, .. }, .. } = &event.fact else {
                return Err(invalid("source proof is not a message opening"));
            };
            maka_runtime::message::validate_sources(content, source_messages).map_err(invalid)?;
            if event.invocation.session_id != session { return Err(invalid("source proof Session changed")); }
            let index = source_messages.iter().position(|s| s.message.message_id == message)
                .ok_or_else(|| invalid("source proof identity changed"))?;
            Ok(Some(RootMessageProof { opening: StoredEvent { sequence: sequence_number(sequence)?, event }, index }))
        })).await
    }
}
fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
