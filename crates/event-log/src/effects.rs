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

//! One-shot Host resource facts outside an Agent invocation. Only the Host
//! adapter writes these records; plugin business state uses separate storage.

use crate::{EventLog, StoreError};
use maka_plugins::{call::Identity, fiber, storage::Namespace};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection, Row};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "input", rename_all = "snake_case")]
pub enum Operation {
    Notification {
        input: maka_plugins::client_capability::Notification,
        provider: maka_runtime::capability::Identity,
        registration_id: String,
    },
    File(maka_plugins::filesystem::Operation),
    Model {
        input: maka_plugins::llm::Generate,
        model: maka_runtime::execution::ModelBinding,
        thinking_level: Option<maka_runtime::execution::ThinkingLevel>,
    },
    Client {
        input: maka_plugins::client_capability::Call,
        provider: maka_runtime::capability::Identity,
        registration_id: String,
        offer_id: String,
        tool_call_id: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub source: Identity,
    pub owner: fiber::Identity,
    pub boundary: maka_plugins::authorization::Boundary,
    pub operation: Operation,
}
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Outcome {
    Completed { value: Value },
    Image { mime_type: String, digest: String },
    Failed { message: String },
    Unknown { message: String },
}
pub struct Record {
    pub request: Request,
    pub outcome: Option<Outcome>,
    pub payload: Option<Vec<u8>>,
}
impl EventLog {
    /// Caller may start the effect only after this durable admission succeeds.
    pub async fn begin_host_effect(&self, request: Request) -> Result<Uuid, StoreError> {
        self.validate_root()?;
        if matches!(request.source, Identity::Agent { .. }) {
            return Err(invalid("Agent effects belong to their invocation journal"));
        }
        let namespace = Namespace::new(
            request.owner.package_id.clone(),
            request.owner.scope.clone(),
        )
        .map_err(|error| invalid(error.to_string()))?;
        let raw = serde_json::to_string(&request)?;
        let id = Uuid::new_v4();
        self.connection.run(move |connection| {
            Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            sqlx::query("INSERT INTO host_effects (id, package_id, scope_id, request) VALUES (?, ?, ?, ?)")
                .bind(id.to_string()).bind(namespace.package()).bind(String::from(namespace.scope().clone())).bind(raw)
                .execute(&mut *tx).await?;
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            Ok(id)
            })
        }).await
    }

    /// Settlement cannot overwrite a known outcome, including recovery's unknown.
    pub async fn settle_host_effect(
        &self,
        id: Uuid,
        outcome: Outcome,
        payload: Option<Vec<u8>>,
    ) -> Result<(), StoreError> {
        self.validate_root()?;
        match (&outcome, &payload) {
            (Outcome::Image { mime_type, digest }, Some(bytes))
                if matches!(
                    mime_type.as_str(),
                    "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                ) && &maka_runtime::artifact::content_digest(bytes) == digest => {}
            (Outcome::Image { .. }, _) | (_, Some(_)) => {
                return Err(invalid("invalid effect image payload"));
            }
            _ => {}
        }
        let raw = serde_json::to_string(&outcome)?;
        self.connection.run(move |connection| {
            Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let result = sqlx::query("UPDATE host_effects SET outcome = ?, payload = ? WHERE id = ? AND outcome IS NULL")
                .bind(raw).bind(payload).bind(id.to_string()).execute(&mut *tx).await?;
            if result.rows_affected() != 1 {
                return Err(StoreError::EventConflict);
            }
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            Ok(())
            })
        }).await
    }

    pub async fn host_effect(
        &self,
        namespace: Namespace,
        id: Uuid,
    ) -> Result<Option<Record>, StoreError> {
        self.validate_root()?;
        self.connection.run(move |connection| {
            Box::pin(async move {
            let row = sqlx::query("SELECT request, outcome, payload FROM host_effects WHERE id = ? AND package_id = ? AND scope_id = ?")
                .bind(id.to_string()).bind(namespace.package()).bind(String::from(namespace.scope().clone()))
                .fetch_optional(connection).await?;
            row.map(|row| {
                Ok(Record {
                    request: serde_json::from_str(row.try_get("request")?)?,
                    outcome: row.try_get::<Option<&str>, _>("outcome")?.map(serde_json::from_str).transpose()?,
                    payload: row.try_get("payload")?,
                })
            }).transpose()
            })
        }).await
    }

    /// Called only at exclusive Host recovery, before accepting new resource work.
    pub async fn recover_host_effects(&self) -> Result<(), StoreError> {
        self.validate_root()?;
        let outcome = serde_json::to_string(&Outcome::Unknown {
            message:
                "Host stopped before recording the effect outcome; do not replay automatically"
                    .into(),
        })?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    sqlx::query("UPDATE host_effects SET outcome = ? WHERE outcome IS NULL")
                        .bind(outcome)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    Ok(())
                })
            })
            .await
    }
}
fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
