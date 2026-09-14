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

//! Append-only requests/outcomes and atomically derived Session grants.
pub(crate) mod lifecycle;
mod records;

use crate::{EventLog, StoreError};
use maka_runtime::interaction::{
    Decision, GrantTarget, InteractionOutcome, InteractionRecord, InteractionRequest, SessionGrant,
    entity_id,
};
use records::{invalid, read};
use sqlx::Connection;

/// A conflicting retry returns the actual canonical fact, never the candidate.
pub struct InteractionCommit {
    pub matches: bool,
    pub record: InteractionRecord,
}

impl EventLog {
    pub async fn establish_interaction(
        &self,
        request: &InteractionRecord,
    ) -> Result<InteractionCommit, StoreError> {
        self.validate_root()?;
        request.validate().map_err(invalid)?;
        if request.outcome.is_some() {
            return Err(invalid("request establishment cannot include an outcome"));
        }
        let encoded = records::encode(request, 20 * 1024)?;
        let request = request.clone();
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    if let Some(mut record) = read(&mut tx, &request.request_id).await? {
                        let outcome = record.outcome.take();
                        let matches = record == request;
                        record.outcome = outcome;
                        return Ok(InteractionCommit { matches, record });
                    }
                    sqlx::query("INSERT INTO interaction_requests VALUES (?, ?, ?, ?)")
                        .bind(&request.request_id)
                        .bind(&request.session_id)
                        .bind(request.created_at as i64)
                        .bind(encoded)
                        .execute(&mut *tx)
                        .await?;
                    crate::sessions::advance_revision(&mut tx, &request.session_id).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_modify(|_| {});
                    Ok(InteractionCommit {
                        matches: true,
                        record: request,
                    })
                })
            })
            .await
    }

    /// First outcome wins. Only that outcome can create its matching grant.
    /// Callers cannot accidentally grant another provider, scope, or Session.
    pub async fn commit_interaction_outcome(
        &self,
        request_id: &str,
        outcome: InteractionOutcome,
    ) -> Result<InteractionCommit, StoreError> {
        self.validate_root()?;
        entity_id(request_id).map_err(invalid)?;
        outcome.validate().map_err(invalid)?;
        let request_id = request_id.to_owned();
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let mut record = read(&mut tx, &request_id)
                        .await?
                        .ok_or_else(|| invalid("interaction request not found"))?;
                    outcome
                        .validate_for_request(&record.request)
                        .map_err(invalid)?;
                    if let Some(existing) = &record.outcome {
                        return Ok(InteractionCommit {
                            matches: records::equivalent(existing, &outcome),
                            record,
                        });
                    }
                    sqlx::query("INSERT INTO interaction_outcomes VALUES (?, ?)")
                        .bind(&request_id)
                        .bind(records::encode(&outcome, 8 * 1024)?)
                        .execute(&mut *tx)
                        .await?;
                    if let InteractionOutcome::ClientCapabilityDecision {
                        decision: Decision::Allow,
                        committed_at,
                    } = &outcome
                    {
                        let InteractionRequest::ClientCapability { target, .. } = &record.request
                        else {
                            return Err(invalid("grant outcome requires capability request"));
                        };
                        let grant = SessionGrant {
                            session_id: record.session_id.clone(),
                            target: target.clone(),
                            granted_at: *committed_at,
                        };
                        sqlx::query(
                            "INSERT OR IGNORE INTO client_capability_session_grants VALUES (?, ?)",
                        )
                        .bind(records::authority_key(&grant.session_id, &grant.target)?)
                        .bind(records::encode(&grant, 12 * 1024)?)
                        .execute(&mut *tx)
                        .await?;
                    }
                    record.outcome = Some(outcome);
                    crate::sessions::advance_revision(&mut tx, &record.session_id).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_modify(|_| {});
                    Ok(InteractionCommit {
                        matches: true,
                        record,
                    })
                })
            })
            .await
    }

    pub async fn interaction(
        &self,
        request_id: &str,
    ) -> Result<Option<InteractionRecord>, StoreError> {
        self.validate_root()?;
        entity_id(request_id).map_err(invalid)?;
        let request_id = request_id.to_owned();
        self.connection
            .run(move |connection| Box::pin(async move { read(connection, &request_id).await }))
            .await
    }

    pub async fn pending_interactions(
        &self,
        session_id: &str,
    ) -> Result<Vec<InteractionRecord>, StoreError> {
        self.validate_root()?;
        entity_id(session_id).map_err(invalid)?;
        let session_id = session_id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let result = pending(&mut tx, &session_id).await?;
                    tx.commit().await?;
                    Ok(result)
                })
            })
            .await
    }

    pub async fn client_capability_grant(
        &self,
        session_id: &str,
        target: &GrantTarget,
    ) -> Result<Option<SessionGrant>, StoreError> {
        self.validate_root()?;
        let key = records::authority_key(session_id, target)?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let raw: Option<String> = sqlx::query_scalar(
                "SELECT record_json FROM client_capability_session_grants WHERE authority_key = ?"
            ).bind(&key).fetch_optional(connection).await?;
                    raw.map(|raw| {
                        let grant: SessionGrant = records::decode(&raw, 12 * 1024)?;
                        grant.validate().map_err(invalid)?;
                        if records::authority_key(&grant.session_id, &grant.target)? != key {
                            return Err(invalid("stored Session grant authority changed"));
                        }
                        Ok(grant)
                    })
                    .transpose()
                })
            })
            .await
    }
}

/// Read inside the caller's observation transaction for a coherent projection.
pub(crate) async fn pending(
    connection: &mut sqlx::SqliteConnection,
    session_id: &str,
) -> Result<Vec<InteractionRecord>, StoreError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT request_id FROM interaction_requests AS request WHERE session_id = ?
         AND NOT EXISTS (SELECT 1 FROM interaction_outcomes AS outcome
                         WHERE outcome.request_id = request.request_id)
         ORDER BY created_at, request_id LIMIT 17",
    )
    .bind(session_id)
    .fetch_all(&mut *connection)
    .await?;
    if ids.len() > 16 {
        return Err(invalid(
            "pending interactions exceed Session projection capacity",
        ));
    }
    let mut records = Vec::with_capacity(ids.len());
    for id in ids {
        records.push(
            read(connection, &id)
                .await?
                .ok_or_else(|| invalid("stored interaction disappeared"))?,
        );
    }
    Ok(records)
}
