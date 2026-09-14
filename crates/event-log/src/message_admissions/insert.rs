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

use super::{MAX_PENDING_BYTES, PendingMessageAdmission, invalid, read};
use crate::StoreError;
use maka_runtime::{event::Invocation, message::MessageDisposition};
use sqlx::SqliteConnection;

pub(super) enum Owner {
    Unsealed,
    /// Host still owns this latest Run's cleanup/handoff under its admission gate.
    Current,
}
pub(super) async fn insert(
    tx: &mut SqliteConnection,
    admission: &PendingMessageAdmission,
    owner: Owner,
) -> Result<Option<u64>, StoreError> {
    admission.validate()?;
    if admission.steering_invocation.is_some() {
        return Err(invalid("new admission cannot override steering ownership"));
    }
    let encoded = serde_json::to_string(&admission)?;
    if encoded.len() > 1024 * 1024 {
        return Err(StoreError::PrefixTooLarge);
    }
    let session = &admission.invocation.session_id;
    let message = &admission.source.message.message_id;
    if let Some(previous) = read(&mut *tx, session, message).await? {
        if previous != *admission {
            return Err(invalid("message admission identity changed"));
        }
        return Ok(None);
    }
    let archived: Option<bool> =
        sqlx::query_scalar("SELECT archived FROM session_control WHERE id = ?")
            .bind(session)
            .fetch_optional(&mut *tx)
            .await?;
    match archived {
        None => return Err(StoreError::SessionNotFound),
        Some(true) => return Err(invalid("cannot admit a message to an archived Session")),
        Some(false) => {}
    }
    let delivered: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM message_sources WHERE session_id = ? AND message_id = ?)",
    )
    .bind(session)
    .bind(message)
    .fetch_one(&mut *tx)
    .await?;
    if delivered {
        return Err(invalid("message already has an immutable delivery"));
    }
    if crate::message_queue::cancelled(&mut *tx, session, message).await? {
        return Err(invalid("message has a durable cancellation"));
    }
    crate::message_identity::validate_source_id(&mut *tx, session, message).await?;
    if crate::message_interrupts::fenced(&mut *tx, session, &admission.invocation.run_id).await? {
        return Err(StoreError::SessionBusy);
    }
    let active: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(o.event_json, '$.invocation') FROM runtime_events o
         WHERE o.kind = 'invocation_opened'
         AND json_extract(o.event_json, '$.invocation.session_id') = ?
         AND (?2 OR NOT EXISTS(SELECT 1 FROM runtime_events t
            WHERE t.invocation_id = o.invocation_id AND t.kind = 'invocation_ended'))
         ORDER BY o.sequence DESC LIMIT 1",
    )
    .bind(session)
    .bind(matches!(owner, Owner::Current))
    .fetch_optional(&mut *tx)
    .await?;
    match (admission.source.disposition, active) {
        (MessageDisposition::TurnStarted, None) => {}
        (MessageDisposition::Steering | MessageDisposition::Followup, Some(active))
            if serde_json::from_str::<Invocation>(&active)? == admission.invocation => {}
        _ => {
            return Err(invalid(
                "message admission no longer matches the active Run",
            ));
        }
    }
    let (count, bytes, position): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(length(CAST(record_json AS BLOB))),0), COALESCE(MAX(position),0)
         FROM message_admissions WHERE session_id = ?"
    ).bind(session).fetch_one(&mut *tx).await?;
    if count >= 64 || bytes + encoded.len() as i64 > MAX_PENDING_BYTES {
        return Err(StoreError::PrefixTooLarge);
    }
    let position = position
        .checked_add(1)
        .ok_or_else(|| invalid("message queue position exhausted"))?;
    if admission.source.disposition == MessageDisposition::TurnStarted && count != 0 {
        return Err(StoreError::SessionBusy);
    }
    sqlx::query("INSERT INTO message_admissions (session_id, message_id, position, record_json) VALUES (?, ?, ?, ?)")
        .bind(session).bind(message).bind(position).bind(encoded).execute(&mut *tx).await?;
    crate::message_queue::bump(tx, session).await.map(Some)
}
