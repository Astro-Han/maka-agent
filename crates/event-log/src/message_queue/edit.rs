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

use super::{MessageQueue, QueueChange, QueueEdit, invalid};
use crate::{StoreError, message_admissions::PendingMessageAdmission};
use maka_runtime::message::MessageDisposition as Disposition;
use sqlx::SqliteConnection;
use std::collections::HashSet;

pub(crate) struct Applied {
    pub result: QueueChange,
    pub changed: bool,
}

pub(crate) async fn apply(
    tx: &mut SqliteConnection,
    session: &str,
    mut queue: MessageQueue,
    edit: QueueEdit,
) -> Result<Applied, StoreError> {
    let original = queue.entries.clone();
    let mut retracted = Vec::new();
    let mut cancellation = None;
    match edit {
        QueueEdit::RetractAll { cancellation_id } => {
            crate::sessions::validate_id(&cancellation_id)?;
            cancellation = Some(cancellation_id);
            queue.entries.retain(|entry| {
                if entry.source.disposition == Disposition::TurnStarted {
                    return true;
                }
                retracted.push(entry.clone());
                false
            });
        }
        QueueEdit::Retract {
            message_id,
            cancellation_id,
        } => {
            crate::sessions::validate_id(&cancellation_id)?;
            let index = queued(&queue, &message_id)?;
            cancellation = Some(cancellation_id);
            retracted.push(queue.entries.remove(index));
        }
        QueueEdit::Promote {
            message_id,
            invocation,
        } => {
            if invocation.session_id != session {
                return Err(invalid("promotion Session changed"));
            }
            let index = queued(&queue, &message_id)?;
            if queue.entries[index].source.disposition != Disposition::Followup {
                return Err(invalid("message is already steering"));
            }
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_events o WHERE o.kind = 'invocation_opened'
                 AND o.invocation_id = ? AND json_extract(o.event_json, '$.invocation') = json(?)
                 AND NOT EXISTS(SELECT 1 FROM runtime_events t WHERE t.invocation_id = o.invocation_id AND t.kind = 'invocation_ended'))"
            ).bind(&invocation.invocation_id).bind(serde_json::to_string(&invocation)?)
                .fetch_one(&mut *tx).await?;
            if !active {
                return Err(invalid("no active Run can accept steering"));
            }
            let mut entry = queue.entries.remove(index);
            entry.steering_invocation = (entry.invocation != invocation).then_some(invocation);
            // Original submit placement remains unchanged. The current disposition
            // changes with promotion, as in the source admission contract.
            entry.source.disposition = Disposition::Steering;
            let after = queue
                .entries
                .iter()
                .rposition(|e| e.source.disposition == Disposition::Steering)
                .map_or(0, |index| index + 1);
            queue.entries.insert(after, entry);
        }
        QueueEdit::Update {
            message_id,
            content,
            skill_invocation,
            required_tools,
        } => {
            let index = queued(&queue, &message_id)?;
            let entry = &mut queue.entries[index];
            entry.source.message.content = *content;
            // Edits change delivery, not the original submission identity used
            // to prove admission after an Epoch change.
            entry.source.skill_invocation = skill_invocation;
            entry.required_tools = required_tools;
            entry.source.validate().map_err(invalid)?;
        }
        QueueEdit::Reorder { message_ids } => {
            if message_ids.len() > 64
                || message_ids.iter().collect::<HashSet<_>>().len() != message_ids.len()
            {
                return Err(invalid("invalid message reorder identities"));
            }
            let lane = message_ids
                .first()
                .and_then(|id| {
                    queue
                        .entries
                        .iter()
                        .find(|e| &e.source.message.message_id == id)
                })
                .map_or(Disposition::Followup, |e| e.source.disposition);
            if lane == Disposition::TurnStarted {
                return Err(invalid("root handoff cannot be reordered"));
            }
            let indexes: Vec<_> = queue
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| (e.source.disposition == lane).then_some(i))
                .collect();
            if indexes.len() != message_ids.len() {
                return Err(invalid("message queue changed before reorder"));
            }
            let reordered: Result<Vec<_>, _> = message_ids
                .iter()
                .map(|id| {
                    queue
                        .entries
                        .iter()
                        .find(|e| {
                            e.source.disposition == lane && &e.source.message.message_id == id
                        })
                        .cloned()
                        .ok_or_else(|| invalid("message queue changed before reorder"))
                })
                .collect();
            for (index, entry) in indexes.into_iter().zip(reordered?) {
                queue.entries[index] = entry;
            }
        }
    }
    let changed = queue.entries != original;
    if changed {
        let bytes = queue.entries.iter().try_fold(0usize, |sum, entry| {
            let bytes = serde_json::to_vec(entry)?.len();
            if bytes > 1024 * 1024 {
                return Err(StoreError::PrefixTooLarge);
            }
            Ok(sum.saturating_add(bytes))
        })?;
        if bytes > 8 * 1024 * 1024 {
            return Err(StoreError::PrefixTooLarge);
        }
        for entry in &retracted {
            sqlx::query("INSERT INTO message_cancellations(session_id, message_id, cancellation_id) VALUES (?, ?, ?)")
                .bind(session).bind(&entry.source.message.message_id)
                .bind(cancellation.as_deref().expect("retraction claim")).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM message_admissions WHERE session_id = ? AND message_id = ?")
                .bind(session)
                .bind(&entry.source.message.message_id)
                .execute(&mut *tx)
                .await?;
        }
        for (index, entry) in queue.entries.iter().enumerate() {
            sqlx::query("UPDATE message_admissions SET position = ?, record_json = ? WHERE session_id = ? AND message_id = ?")
                .bind(index as i64 + 1).bind(serde_json::to_string(entry)?)
                .bind(session).bind(&entry.source.message.message_id).execute(&mut *tx).await?;
        }
    }
    Ok(Applied {
        result: QueueChange { queue, retracted },
        changed,
    })
}

fn queued(queue: &MessageQueue, id: &str) -> Result<usize, StoreError> {
    crate::sessions::validate_id(id)?;
    queue
        .entries
        .iter()
        .position(|e: &PendingMessageAdmission| {
            e.source.message.message_id == id && e.source.disposition != Disposition::TurnStarted
        })
        .ok_or_else(|| invalid("message is not queued"))
}
