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

use maka_runtime::event::{
    CommitError, CommitFuture, EventSink, EventWrite, Fact, InvocationOutcome,
};
use sqlx::{Connection, SqliteConnection};

use crate::{EventLog, StoreError, sequence_number};

impl EventLog {
    /// Commit ordered facts atomically, validating each against the preceding
    /// facts in this transaction. Exact replays return their original sequence.
    /// This must not enclose effects: live dispatch and outcome remain separate.
    pub async fn append_batch(&self, events: &[EventWrite]) -> Result<Vec<u64>, CommitError> {
        self.append_batch_checked(events)
            .await
            .map_err(|error| match error {
                StoreError::CommitUnknown(error) => CommitError::OutcomeUnknown(error.to_string()),
                StoreError::OperationUnknown => CommitError::OutcomeUnknown(error.to_string()),
                other => CommitError::Rejected(other.to_string()),
            })
    }

    async fn append_batch_checked(&self, events: &[EventWrite]) -> Result<Vec<u64>, StoreError> {
        self.validate_root()?;
        let events = events.to_vec();
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let mut sequences = Vec::with_capacity(events.len());
                    let mut last_inserted = None;
                    for event in &events {
                        let result = Self::append_in_transaction(&mut transaction, event).await?;
                        let sequence = match result {
                            AppendResult::Existing(sequence) => sequence,
                            AppendResult::Inserted(sequence) => {
                                last_inserted = Some(sequence);
                                sequence
                            }
                        };
                        sequences.push(sequence);
                    }
                    crate::context::validate_batch(&mut transaction, &events).await?;
                    if let Some(sequence) = last_inserted {
                        transaction
                            .commit()
                            .await
                            .map_err(StoreError::CommitUnknown)?;
                        commits.send_replace(sequence);
                    } else {
                        transaction.rollback().await?;
                    }
                    Ok(sequences)
                })
            })
            .await
    }

    pub(crate) async fn append_in_transaction(
        transaction: &mut SqliteConnection,
        write: &EventWrite,
    ) -> Result<AppendResult, StoreError> {
        let event = write.event();
        let json = serde_json::to_string(event)?;
        let previous: Option<(i64, String)> =
            sqlx::query_as("SELECT sequence, event_json FROM runtime_events WHERE event_id = ?")
                .bind(&event.id)
                .fetch_optional(&mut *transaction)
                .await?;
        if let Some((sequence, previous)) = previous {
            if previous != json {
                return Err(StoreError::EventConflict);
            }
            crate::tool_payloads::verify_replay(transaction, write).await?;
            crate::archive::verify_replay(transaction, event).await?;
            return Ok(AppendResult::Existing(sequence_number(sequence)?));
        }
        let id = &event.invocation.invocation_id;
        let sealed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id = ? AND kind = 'invocation_ended')",
        ).bind(id).fetch_one(&mut *transaction).await?;
        if sealed {
            return Err(StoreError::Sealed);
        }
        let opening: Option<String> = sqlx::query_scalar(
            "SELECT event_json FROM runtime_events WHERE invocation_id = ? AND kind = 'invocation_opened'",
        ).bind(id).fetch_optional(&mut *transaction).await?;
        match (&event.fact, opening) {
            (Fact::InvocationOpened { .. }, None) => {
                let archived: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM session_control WHERE id = ? AND archived = 1)",
                )
                .bind(&event.invocation.session_id)
                .fetch_one(&mut *transaction)
                .await?;
                if archived {
                    return Err(StoreError::InvalidTransition(
                        "cannot admit an archived session".into(),
                    ));
                }
                let active: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events AS opening
                     WHERE opening.kind = 'invocation_opened'
                     AND json_extract(opening.event_json, '$.invocation.session_id') = ?
                     AND NOT EXISTS(SELECT 1 FROM runtime_events AS terminal
                         WHERE terminal.invocation_id = opening.invocation_id AND terminal.kind = 'invocation_ended'))",
                ).bind(&event.invocation.session_id).fetch_one(&mut *transaction).await?;
                if active {
                    return Err(StoreError::InvalidTransition(
                        "session already has an unsealed invocation".into(),
                    ));
                }
            }
            (Fact::InvocationOpened { .. }, Some(_)) => {
                return Err(StoreError::InvalidTransition(
                    "invocation already opened".into(),
                ));
            }
            (_, None) => {
                return Err(StoreError::InvalidTransition(
                    "invocation not opened".into(),
                ));
            }
            (_, Some(opening)) => {
                if serde_json::from_str::<maka_runtime::event::RuntimeEvent>(&opening)?.invocation
                    != event.invocation
                {
                    return Err(StoreError::InvalidTransition(
                        "invocation identity changed".into(),
                    ));
                }
            }
        }
        crate::tool_calls::validate(transaction, event).await?;
        crate::continuation::validate(transaction, event).await?;
        crate::steering::validate_append(transaction, event).await?;
        crate::message_identity::validate(transaction, event).await?;
        crate::message_admissions::consume(transaction, event).await?;
        crate::workhub::apply(transaction, event).await?;
        crate::context::validate_append(transaction, event).await?;
        crate::archive::validate_append(transaction, event).await?;
        if matches!(event.fact, Fact::InvocationEnded { .. }) {
            crate::interactions::lifecycle::require_closed(transaction, &event.invocation).await?;
        }
        if let Fact::ToolSettled { operation_id, .. } = &event.fact {
            let dispatched: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id = ? AND operation_id = ? AND kind = 'tool_dispatched')",
            ).bind(id).bind(operation_id).fetch_one(&mut *transaction).await?;
            if !dispatched {
                return Err(StoreError::InvalidTransition(
                    "outcome without dispatch".into(),
                ));
            }
        }
        if let Fact::ModelCompleted { step_id, .. }
        | Fact::ModelInterrupted { step_id, .. }
        | Fact::ModelObserved { step_id, .. } = &event.fact
        {
            let requested: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id = ? AND operation_id = ? AND kind = 'model_requested')",
            ).bind(id).bind(step_id).fetch_one(&mut *transaction).await?;
            if !requested {
                return Err(StoreError::InvalidTransition(
                    "model output without request".into(),
                ));
            }
            let settled: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id = ? AND operation_id = ? AND kind IN ('model_completed', 'model_interrupted'))",
            ).bind(id).bind(step_id).fetch_one(&mut *transaction).await?;
            if settled {
                return Err(StoreError::InvalidTransition(
                    "model request already settled".into(),
                ));
            }
        }
        if matches!(
            event.fact,
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Completed
            }
        ) {
            let pending: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_events AS dispatch
                 WHERE dispatch.invocation_id = ? AND dispatch.kind IN ('tool_dispatched', 'model_requested')
                 AND NOT EXISTS(SELECT 1 FROM runtime_events AS outcome
                     WHERE outcome.invocation_id = dispatch.invocation_id
                     AND outcome.operation_id = dispatch.operation_id
                     AND ((dispatch.kind = 'tool_dispatched' AND outcome.kind = 'tool_settled')
                       OR (dispatch.kind = 'model_requested' AND outcome.kind IN ('model_completed','model_interrupted')))))",
            ).bind(id).fetch_one(&mut *transaction).await?;
            if pending {
                return Err(StoreError::InvalidTransition(
                    "cannot complete with an unresolved tool or model outcome".into(),
                ));
            }
        }
        let inserted = sqlx::query(
            "INSERT INTO event_log (event_id, invocation_id, kind, operation_id, event_json)
             SELECT ?1, ?2, ?3, ?4, ?5 WHERE NOT EXISTS (
                 SELECT 1 FROM message_sources WHERE session_id = ?6 AND message_id = ?1
                 UNION ALL SELECT 1 FROM message_admissions WHERE session_id = ?6 AND message_id = ?1)",
        )
        .bind(&event.id)
        .bind(id)
        .bind(event.fact.kind())
        .bind(event.fact.operation_id())
        .bind(json)
        .bind(&event.invocation.session_id)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() != 1 {
            return Err(StoreError::EventConflict);
        }
        let sequence = sequence_number(inserted.last_insert_rowid())?;
        crate::message_sources::insert(transaction, event).await?;
        crate::tool_payloads::insert(transaction, write).await?;
        if matches!(event.fact, Fact::InvocationEnded { .. }) {
            crate::sessions::read_state::mark_unread(
                transaction,
                &event.invocation.session_id,
                &event.invocation.invocation_id,
            )
            .await?;
        }
        if matches!(
            event.fact,
            Fact::InvocationOpened { .. }
                | Fact::MessageSteered { .. }
                | Fact::ModelCompleted { .. }
                | Fact::ModelInterrupted { .. }
                | Fact::InvocationEnded { .. }
        ) {
            crate::sessions::project_execution(transaction, sequence).await?;
            crate::sessions::advance_execution(transaction, &event.invocation.session_id).await?;
        }
        Ok(AppendResult::Inserted(sequence))
    }
}

pub(crate) enum AppendResult {
    Existing(u64),
    Inserted(u64),
}

impl EventLog {
    pub async fn append(&self, event: &EventWrite) -> Result<u64, CommitError> {
        Ok(self.append_batch(std::slice::from_ref(event)).await?[0])
    }
}

impl EventSink for EventLog {
    fn commit(self: std::sync::Arc<Self>, event: EventWrite) -> CommitFuture {
        Box::pin(async move { self.append(&event).await })
    }
}
