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

use super::{AuxiliarySource as Source, Outcome, invalid};
use crate::{EventLog, StoreError};
use maka_runtime::{execution::ModelBinding, model::ModelUsage};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, SqliteConnection};
use std::time::SystemTime;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
pub(super) struct Admission {
    source: Source,
    binding: ModelBinding,
    session_id: Option<String>,
    started_at: SystemTime,
}

#[derive(Serialize)]
struct Settlement {
    request_id: Uuid,
    outcome: Outcome,
    completed_at: SystemTime,
}

#[derive(Serialize)]
struct Observation {
    request_id: Uuid,
    usage: ModelUsage,
}

impl EventLog {
    /// Called inside an already-admitted Host SDK effect, before model dispatch.
    /// The source journal supplies the binding; plugins cannot invent accounting
    /// identity by supplying a Session ID or current provider configuration.
    pub async fn begin_auxiliary_model(
        &self,
        source: Source,
        quote: Option<maka_runtime::pricing::Quote>,
    ) -> Result<Uuid, StoreError> {
        self.validate_root()?;
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let (binding, session_id) = binding(&mut tx, &source).await?;
                    if let Some(quote) = &quote {
                        quote.validate(&binding.model).map_err(invalid)?;
                    }
                    let record = Admission {
                        source,
                        binding,
                        session_id,
                        started_at: SystemTime::now(),
                    };
                    let id = Uuid::new_v4();
                    let sequence = insert(&mut tx, id, "auxiliary_model_started", &record).await?;
                    if let Some(quote) = &quote {
                        super::valuation::capture(&mut tx, &id.to_string(), quote).await?;
                    }
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_replace(sequence);
                    Ok(id)
                })
            })
            .await
    }

    /// Provider-reported usage is durable even if later semantic validation fails.
    pub async fn observe_auxiliary_model(
        &self,
        id: Uuid,
        usage: ModelUsage,
    ) -> Result<(), StoreError> {
        self.validate_root()?;
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    require_pending(&mut tx, id).await?;
                    super::valuation::value(&mut tx, &id.to_string(), &usage).await?;
                    let sequence = insert(
                        &mut tx,
                        Uuid::new_v4(),
                        "auxiliary_model_usage",
                        &Observation {
                            request_id: id,
                            usage,
                        },
                    )
                    .await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_replace(sequence);
                    Ok(())
                })
            })
            .await
    }

    pub async fn settle_auxiliary_model(
        &self,
        id: Uuid,
        outcome: Outcome,
    ) -> Result<(), StoreError> {
        self.validate_root()?;
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    require_pending(&mut tx, id).await?;
                    let sequence = settle(&mut tx, id, outcome).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_replace(sequence);
                    Ok(())
                })
            })
            .await
    }

    /// Exclusive Host startup only; no in-memory provider request survives its
    /// previous owner. A missing result is unknown, never free or safe to replay.
    pub async fn recover_auxiliary_models(&self) -> Result<(), StoreError> {
        self.validate_root()?;
        loop {
            let commits = self.commits.clone();
            let count = self.connection.run(move |connection| Box::pin(async move {
                let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                let ids: Vec<String> = sqlx::query_scalar(
                    "SELECT event_id FROM event_log started
                     WHERE kind = 'auxiliary_model_started' AND NOT EXISTS (
                        SELECT 1 FROM event_log settled WHERE settled.kind = 'auxiliary_model_settled'
                        AND json_extract(settled.event_json, '$.request_id') = started.event_id)
                     ORDER BY sequence LIMIT 128"
                ).fetch_all(&mut *tx).await?;
                let mut last = None;
                for id in &ids {
                    last = Some(settle(&mut tx, Uuid::parse_str(id)
                        .map_err(|_| invalid("invalid auxiliary request identity"))?, Outcome::Unknown).await?);
                }
                tx.commit().await.map_err(StoreError::CommitUnknown)?;
                if let Some(sequence) = last { commits.send_replace(sequence); }
                Ok(ids.len())
            })).await?;
            if count < 128 {
                return Ok(());
            }
        }
    }
}

async fn binding(
    connection: &mut SqliteConnection,
    source: &Source,
) -> Result<(ModelBinding, Option<String>), StoreError> {
    match source {
        Source::Agent {
            invocation,
            operation_id,
        } => {
            let raw: Option<String> = sqlx::query_scalar(
                "SELECT opening.event_json FROM runtime_events opening
                 JOIN runtime_events dispatch ON dispatch.invocation_id = opening.invocation_id
                 WHERE opening.kind = 'invocation_opened' AND opening.invocation_id = ?1
                 AND dispatch.kind = 'tool_dispatched' AND dispatch.operation_id = ?2
                 AND json_extract(dispatch.event_json, '$.fact.name') = 'llm.generate'
                 AND json_extract(dispatch.event_json, '$.fact.call.origin.kind') = 'host_sdk'
                 AND NOT EXISTS (SELECT 1 FROM runtime_events settled
                    WHERE (settled.kind = 'tool_settled' AND settled.operation_id = ?2)
                       OR (settled.kind = 'invocation_ended' AND settled.invocation_id = ?1))",
            )
            .bind(&invocation.invocation_id)
            .bind(operation_id)
            .fetch_optional(connection)
            .await?;
            let event: maka_runtime::event::RuntimeEvent = serde_json::from_str(
                &raw.ok_or_else(|| invalid("auxiliary model source is not pending"))?,
            )?;
            if event.invocation != *invocation {
                return Err(invalid("auxiliary model invocation changed"));
            }
            match event.fact {
                maka_runtime::event::Fact::InvocationOpened {
                    configuration: Some(config),
                    ..
                } => config
                    .model
                    .map(|binding| (binding, Some(invocation.session_id.clone())))
                    .ok_or_else(|| invalid("auxiliary model has no frozen binding")),
                _ => Err(invalid("auxiliary model has no frozen configuration")),
            }
        }
        Source::HostEffect { id } => {
            let raw: Option<String> = sqlx::query_scalar(
                "SELECT request FROM host_effects WHERE id = ? AND outcome IS NULL",
            )
            .bind(id.to_string())
            .fetch_optional(connection)
            .await?;
            let request: crate::effects::Request = serde_json::from_str(
                &raw.ok_or_else(|| invalid("auxiliary Host effect is not pending"))?,
            )?;
            let session = match request.boundary {
                maka_plugins::authorization::Boundary::Session { boundary, .. } => {
                    Some(boundary.session_id)
                }
                _ => None,
            };
            match request.operation {
                crate::effects::Operation::Model { model, .. } => Ok((model, session)),
                _ => Err(invalid("Host effect is not a model operation")),
            }
        }
    }
}

async fn require_pending(connection: &mut SqliteConnection, id: Uuid) -> Result<(), StoreError> {
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM event_log WHERE event_id = ?1 AND kind = 'auxiliary_model_started')
         AND NOT EXISTS(SELECT 1 FROM event_log WHERE kind = 'auxiliary_model_settled'
             AND json_extract(event_json, '$.request_id') = ?1)"
    ).bind(id.to_string()).fetch_one(connection).await?;
    if pending {
        Ok(())
    } else {
        Err(invalid("auxiliary model request is not pending"))
    }
}

async fn settle(
    connection: &mut SqliteConnection,
    id: Uuid,
    outcome: Outcome,
) -> Result<u64, StoreError> {
    insert(
        connection,
        Uuid::new_v4(),
        "auxiliary_model_settled",
        &Settlement {
            request_id: id,
            outcome,
            completed_at: SystemTime::now(),
        },
    )
    .await
}

async fn insert(
    connection: &mut SqliteConnection,
    id: Uuid,
    kind: &str,
    record: &impl Serialize,
) -> Result<u64, StoreError> {
    let inserted =
        sqlx::query("INSERT INTO event_log(event_id, kind, event_json) VALUES (?, ?, ?)")
            .bind(id.to_string())
            .bind(kind)
            .bind(serde_json::to_string(record)?)
            .execute(connection)
            .await?;
    crate::sequence_number(inserted.last_insert_rowid())
}
