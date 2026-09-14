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

//! Durable pending messages. Consuming one is part of the canonical event transaction.
use crate::{EventLog, StoreError};
use maka_runtime::{
    event::{Fact, Invocation, RuntimeEvent},
    message::{MessageDisposition, RootSourceMessage},
};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, SqliteConnection};

pub(crate) mod insert;
mod receipts;
pub use receipts::MessageSubmitReceipt;

const MAX_PENDING_BYTES: i64 = 8 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMessageAdmission {
    pub invocation: Invocation,
    /// Promotion may target a later Run without rewriting original admission ownership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_invocation: Option<Invocation>,
    pub source: RootSourceMessage,
    /// Frozen prerequisites for delivery or promotion; never re-read Skills
    /// while handing an accepted message to another Run.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub required_tools: std::collections::BTreeSet<String>,
    pub admitted_at: u64,
}
impl PendingMessageAdmission {
    pub fn steering_target(&self) -> &Invocation {
        self.steering_invocation
            .as_ref()
            .unwrap_or(&self.invocation)
    }
    fn validate(&self) -> Result<(), StoreError> {
        for id in [
            &self.invocation.session_id,
            &self.invocation.turn_id,
            &self.invocation.run_id,
            &self.invocation.invocation_id,
        ] {
            crate::sessions::validate_id(id)?;
        }
        self.source.validate().map_err(invalid)?;
        if let Some(target) = &self.steering_invocation {
            if self.source.disposition != MessageDisposition::Steering
                || target.session_id != self.invocation.session_id
            {
                return Err(invalid("invalid promoted steering target"));
            }
            for id in [&target.turn_id, &target.run_id, &target.invocation_id] {
                crate::sessions::validate_id(id)?;
            }
        }
        if self.source.submitted_intent.is_some()
            && self.source.disposition != MessageDisposition::TurnStarted
        {
            return Err(invalid(
                "exact Turn intent cannot enter a running Turn queue",
            ));
        }
        if self.admitted_at > maka_runtime::configuration::validation::MAX_SAFE_INTEGER {
            return Err(invalid("invalid message admission time"));
        }
        Ok(())
    }
}
impl EventLog {
    /// Startup keyset scan; never materialize every Session's pending content.
    pub async fn pending_message_sessions(
        &self,
        after: Option<&str>,
    ) -> Result<Vec<String>, StoreError> {
        self.validate_root()?;
        if let Some(after) = after {
            crate::sessions::validate_id(after)?;
        }
        let after = after.map(str::to_owned);
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    Ok(sqlx::query_scalar(
                        "SELECT DISTINCT session_id FROM message_admissions
                 WHERE (?1 IS NULL OR session_id > ?1) ORDER BY session_id LIMIT 64",
                    )
                    .bind(after)
                    .fetch_all(connection)
                    .await?)
                })
            })
            .await
    }

    /// Caller owns Session admission; the independent SQL owner survives waiter cancellation.
    pub async fn admit_message(
        &self,
        admission: PendingMessageAdmission,
    ) -> Result<PendingMessageAdmission, StoreError> {
        self.validate_root()?;
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    if insert::insert(&mut tx, &admission, insert::Owner::Unsealed)
                        .await?
                        .is_some()
                    {
                        tx.commit().await.map_err(StoreError::CommitUnknown)?;
                        commits.send_modify(|_| {});
                    }
                    Ok(admission)
                })
            })
            .await
    }

    pub async fn message_admission(
        &self,
        session: &str,
        message: &str,
    ) -> Result<Option<PendingMessageAdmission>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session)?;
        crate::sessions::validate_id(message)?;
        let session = session.to_owned();
        let message = message.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move { read(connection, &session, &message).await })
            })
            .await
    }

    pub async fn pending_messages(
        &self,
        session: &str,
    ) -> Result<Vec<PendingMessageAdmission>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session)?;
        let session = session.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    pending(&mut tx, &session).await
                })
            })
            .await
    }
}

pub(super) async fn pending(
    tx: &mut SqliteConnection,
    session: &str,
) -> Result<Vec<PendingMessageAdmission>, StoreError> {
    let (count, bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(length(CAST(record_json AS BLOB))),0) FROM message_admissions WHERE session_id = ?"
    ).bind(session).fetch_one(&mut *tx).await?;
    if count > 64 || bytes > MAX_PENDING_BYTES {
        return Err(StoreError::PrefixTooLarge);
    }
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT message_id, record_json FROM message_admissions WHERE session_id = ? ORDER BY position, message_id"
    ).bind(session).fetch_all(tx).await?;
    rows.into_iter()
        .map(|(id, json)| decode(&json, session, &id))
        .collect()
}

async fn read(
    connection: &mut SqliteConnection,
    session: &str,
    message: &str,
) -> Result<Option<PendingMessageAdmission>, StoreError> {
    let json: Option<Option<String>> = sqlx::query_scalar(
        "SELECT CASE WHEN length(CAST(record_json AS BLOB)) <= 1048576 THEN record_json END
         FROM message_admissions WHERE session_id = ? AND message_id = ?",
    )
    .bind(session)
    .bind(message)
    .fetch_optional(connection)
    .await?;
    json.map(|json| decode(&json.ok_or(StoreError::PrefixTooLarge)?, session, message))
        .transpose()
}
fn decode(json: &str, session: &str, message: &str) -> Result<PendingMessageAdmission, StoreError> {
    let record: PendingMessageAdmission = serde_json::from_str(json)?;
    record.validate()?;
    if record.invocation.session_id != session || record.source.message.message_id != message {
        return Err(invalid("message admission identity differs from its key"));
    }
    Ok(record)
}

/// No handoff flag: only the atomic appearance of the canonical delivery retires pending work.
pub(crate) async fn consume(
    connection: &mut SqliteConnection,
    event: &RuntimeEvent,
) -> Result<(), StoreError> {
    for id in crate::message_sources::identities(event) {
        if crate::message_queue::cancelled(connection, &event.invocation.session_id, id).await? {
            return Err(invalid("cancelled message cannot be delivered"));
        }
        let Some(pending) = read(connection, &event.invocation.session_id, id).await? else {
            continue;
        };
        let matches = match &event.fact {
            Fact::MessageSteered {
                message,
                skill_invocation,
            } => {
                *pending.steering_target() == event.invocation
                    && pending.source.disposition == MessageDisposition::Steering
                    && pending.source.submitted_intent.is_none()
                    && pending.source.message == **message
                    && pending.source.skill_invocation == *skill_invocation
            }
            Fact::InvocationOpened { .. } => {
                crate::message_sources::roots(event).iter().any(|source| {
                    source == &pending.source
                        && (source.disposition != MessageDisposition::TurnStarted
                            || pending.invocation == event.invocation)
                })
            }
            _ => false,
        };
        if !matches {
            return Err(invalid(
                "canonical delivery differs from its pending admission",
            ));
        }
        sqlx::query("DELETE FROM message_admissions WHERE session_id = ? AND message_id = ?")
            .bind(&event.invocation.session_id)
            .bind(id)
            .execute(&mut *connection)
            .await?;
        crate::message_queue::bump(connection, &event.invocation.session_id).await?;
    }
    Ok(())
}
fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
