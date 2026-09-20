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

use super::{Assignment, assignment, control, invalid};
use crate::{EventLog, StoreError};
use maka_runtime::workhub::ActionId;
use maka_runtime::{
    session_event::{SessionEvent, SessionFact},
    workhub::{CorrectionAbort, CorrectionIntent, CorrectionRequest, Delegation},
};
use serde::Serialize;
use sqlx::{Connection, SqliteConnection};

mod admit;
mod resolve;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorrectionRecord {
    pub intent: CorrectionIntent,
    pub resolution: Option<CorrectionResolution>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorrectionResolution {
    Assigned(Assignment),
    Aborted(CorrectionAbort),
}

impl EventLog {
    pub async fn workhub_correction(
        &self,
        action: &ActionId,
    ) -> Result<Option<CorrectionRecord>, StoreError> {
        self.validate_root()?;
        let action = action.to_owned();
        self.connection
            .run(move |tx| Box::pin(async move { read(tx, &action).await }))
            .await
    }

    /// Bounded startup work, in durable admission order.
    pub async fn pending_workhub_corrections(&self) -> Result<Vec<CorrectionRecord>, StoreError> {
        self.validate_root()?;
        self.connection.run(move |tx| Box::pin(async move {
            let actions: Vec<String> = sqlx::query_scalar(
                "SELECT action_id FROM workhub_corrections WHERE resolution_kind IS NULL ORDER BY sequence LIMIT 32"
            ).fetch_all(&mut *tx).await?;
            let mut records = Vec::with_capacity(actions.len());
            for action in actions {
                let action = ActionId::new(action).map_err(invalid)?;
                records.push(read(tx, &action).await?.ok_or_else(|| invalid("WorkHub correction disappeared"))?);
            }
            Ok(records)
        })).await
    }

    /// Only this admission requires a live coordinator. Intent and cancellation
    /// of its exact pending Message are indivisible.
    pub async fn request_workhub_correction(
        &self,
        request: CorrectionRequest,
        target_revision: Option<u64>,
        target_owner: Option<maka_runtime::event::Invocation>,
        preparation: &impl Serialize,
    ) -> Result<CorrectionRecord, StoreError> {
        self.validate_root()?;
        request.validate().map_err(invalid)?;
        let preparation = serde_json::to_value(preparation)?;
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    if let Some(previous) = read(&mut tx, &request.action_id).await? {
                        if previous.intent.request != request {
                            return Err(invalid("WorkHub correction identity changed"));
                        }
                        return Ok(previous);
                    }
                    let intent = admit::apply(
                        &mut tx,
                        request,
                        target_revision,
                        target_owner.as_ref(),
                        preparation,
                    )
                    .await?;
                    let sequence = control::append(
                        &mut tx,
                        &SessionEvent::workhub(
                            intent.request.source.turn_id.clone(),
                            SessionFact::WorkhubCorrectionRequested {
                                intent: Box::new(intent.clone()),
                            },
                        ),
                    )
                    .await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_replace(sequence);
                    Ok(CorrectionRecord {
                        intent,
                        resolution: None,
                    })
                })
            })
            .await
    }

    /// Creation, attachment copies, pending delivery and both terminal link facts
    /// share one transaction. This never extends the coordinator's Run.
    pub async fn finish_workhub_correction<T: Serialize>(
        &self,
        action: &ActionId,
        delegation: Delegation,
        configuration: Option<&T>,
    ) -> Result<CorrectionRecord, StoreError> {
        self.validate_root()?;
        let configuration = configuration.map(serde_json::to_string).transpose()?;
        let action = action.to_owned();
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let mut record = required(&mut tx, &action).await?;
                    if record.resolution.is_some() {
                        return Ok(record);
                    }
                    resolve::retired(&mut tx, &record.intent).await?;
                    let assigned =
                        resolve::assign(&mut tx, &record.intent, delegation, configuration).await?;
                    let sequence = control::append(
                        &mut tx,
                        &SessionEvent::workhub(
                            record.intent.request.source.turn_id.clone(),
                            SessionFact::WorkhubSuperseded {
                                action_id: action,
                                replaces_action_id: record
                                    .intent
                                    .request
                                    .replaces_action_id
                                    .clone(),
                                replacement_delegation_id: assigned.id.clone(),
                            },
                        ),
                    )
                    .await?;
                    record.resolution = Some(CorrectionResolution::Assigned(assigned));
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_replace(sequence);
                    Ok(record)
                })
            })
            .await
    }

    pub async fn abort_workhub_correction(
        &self,
        action: &ActionId,
        reason: CorrectionAbort,
    ) -> Result<CorrectionRecord, StoreError> {
        self.validate_root()?;
        let action = action.to_owned();
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let mut record = required(&mut tx, &action).await?;
                    // A retry cannot turn a committed assignment into an abort.
                    if record.resolution.is_some() {
                        return Ok(record);
                    }
                    resolve::retired(&mut tx, &record.intent).await?;
                    let sequence = control::append(
                        &mut tx,
                        &SessionEvent::workhub(
                            record.intent.request.source.turn_id.clone(),
                            SessionFact::WorkhubCorrectionAborted {
                                action_id: action,
                                reason,
                            },
                        ),
                    )
                    .await?;
                    record.resolution = Some(CorrectionResolution::Aborted(reason));
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_replace(sequence);
                    Ok(record)
                })
            })
            .await
    }
}

pub(crate) async fn read(
    tx: &mut SqliteConnection,
    action: &ActionId,
) -> Result<Option<CorrectionRecord>, StoreError> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT CASE WHEN length(CAST(intent_json AS BLOB)) <= 131072 THEN intent_json END,
         resolution_kind, abort_reason FROM workhub_corrections WHERE action_id = ?",
    )
    .bind(action.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((intent, kind, reason)) = row else {
        return Ok(None);
    };
    let intent: CorrectionIntent =
        serde_json::from_str(&intent.ok_or(StoreError::PrefixTooLarge)?)?;
    intent.validate().map_err(invalid)?;
    if &intent.request.action_id != action {
        return Err(invalid("WorkHub correction index changed"));
    }
    let resolution = match kind.as_deref() {
        None => None,
        Some("workhub_superseded") => Some(CorrectionResolution::Assigned(
            assignment::read(tx, action)
                .await?
                .ok_or_else(|| invalid("WorkHub replacement assignment is missing"))?,
        )),
        Some("workhub_correction_aborted") => {
            Some(CorrectionResolution::Aborted(match reason.as_deref() {
                Some("target_unavailable") => CorrectionAbort::TargetUnavailable,
                Some("target_waiting_for_user") => CorrectionAbort::TargetWaitingForUser,
                _ => return Err(invalid("invalid WorkHub correction abort")),
            }))
        }
        _ => return Err(invalid("invalid WorkHub correction resolution")),
    };
    Ok(Some(CorrectionRecord { intent, resolution }))
}

async fn required(
    tx: &mut SqliteConnection,
    action: &ActionId,
) -> Result<CorrectionRecord, StoreError> {
    read(tx, action)
        .await?
        .ok_or_else(|| invalid("WorkHub correction intent is missing"))
}
