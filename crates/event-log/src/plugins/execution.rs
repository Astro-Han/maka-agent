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

use super::invalid;
use crate::{
    EventLog, StoreError,
    message_admissions::{PendingMessageAdmission, insert},
};
use maka_plugins::{
    composition::Scope,
    execution::{Observation, Progress, Receipt, Submit},
    storage::Namespace,
};
use maka_runtime::{
    event::Invocation,
    input::DeliveredMessage,
    message::{MessageDisposition, Placement, RootSourceMessage},
};
use sha2::{Digest, Sha256};
use sqlx::{Connection, SqliteConnection};
use uuid::Uuid;

impl EventLog {
    /// Observe pending admission and canonical delivery in one read transaction.
    pub async fn plugin_execution_progress(
        &self,
        receipt: Receipt,
    ) -> Result<Observation, StoreError> {
        self.validate_root()?;
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin().await?;
            let invocation = &receipt.invocation;
            let boundary = crate::turns::read(&mut tx, &invocation.session_id, Some(&invocation.turn_id)).await?;
            let mut attention_id = None;
            let (progress, terminal_event_id) = match boundary {
                Some(boundary) => {
                    if boundary.root_invocation() != invocation { return Err(StoreError::EventConflict); }
                    use crate::turns::InvocationState;
                    match boundary.state {
                        InvocationState::Admitted | InvocationState::Running => (Progress::Running, None),
                        InvocationState::WaitingForUser => {
                            let mut requests = crate::interactions::pending(&mut tx, &invocation.session_id).await?
                                .into_iter().filter(|request| request.run_id == boundary.invocation.run_id
                                    && request.turn_id == boundary.invocation.turn_id)
                                .map(|request| request.request_id).collect::<Vec<_>>();
                            requests.sort_unstable();
                            attention_id = Some(format!("sha256:{:x}", Sha256::digest(serde_json::to_vec(&requests)?)));
                            (Progress::WaitingForUser, None)
                        }
                        InvocationState::Ended { outcome: maka_runtime::event::InvocationOutcome::HandoffPaused { .. }, event_id } => {
                            attention_id = Some(event_id);
                            (Progress::Paused, None)
                        }
                        InvocationState::Ended { outcome, event_id } => (Progress::Ended { outcome }, Some(event_id)),
                    }
                }
                None => {
                    let pending = crate::message_admissions::read(&mut tx, &invocation.session_id, &receipt.message_id).await?
                        .ok_or_else(|| invalid("accepted execution has no pending or canonical owner"))?;
                    if pending.invocation != *invocation { return Err(StoreError::EventConflict); }
                    (Progress::Pending, None)
                }
            };
            let through_sequence = crate::observation::high_water(&mut tx).await?;
            tx.commit().await?;
            Ok(Observation { receipt, progress, through_sequence, terminal_event_id, attention_id })
        })).await
    }

    /// The caller authorizes the current instance and owns Session admission.
    /// The SQL owner commits accepted work and its stable receipt together even
    /// if the caller loses its response. No plugin generation or Host epoch is a key.
    pub async fn admit_plugin_execution(
        &self,
        namespace: &Namespace,
        request: Submit,
    ) -> Result<Receipt, StoreError> {
        self.validate_root()?;
        request
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        if namespace.scope() == &Scope::DesktopUi {
            return Err(invalid("desktop-ui cannot submit Host execution"));
        }
        let digest = request
            .digest()
            .map_err(|error| invalid(&error.to_string()))?;
        let package = namespace.package().to_owned();
        let scope = String::from(namespace.scope().clone());
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    if let Some(receipt) =
                        read(&mut tx, &package, &scope, &request.operation_id).await?
                    {
                        if receipt.content_digest != digest
                            || receipt.invocation.session_id != request.session_id
                        {
                            return Err(StoreError::EventConflict);
                        }
                        return Ok(receipt);
                    }
                    let receipt = Receipt {
                        invocation: Invocation {
                            session_id: request.session_id,
                            turn_id: Uuid::new_v4().to_string(),
                            run_id: Uuid::new_v4().to_string(),
                            invocation_id: Uuid::new_v4().to_string(),
                        },
                        message_id: Uuid::new_v4().to_string(),
                        content_digest: digest,
                    };
                    let source = RootSourceMessage {
                        message: DeliveredMessage {
                            message_id: receipt.message_id.clone(),
                            submitted_content_digest: request.content.content_digest()?,
                            content: request.content,
                        },
                        submitted_placement: if request.orchestration_mode.is_some() {
                            Placement::CurrentTurn
                        } else {
                            Placement::NextTurn
                        },
                        disposition: MessageDisposition::TurnStarted,
                        skill_invocation: Default::default(),
                        submitted_intent: request.orchestration_mode.map(|mode| {
                            maka_runtime::message::SubmittedTurnIntent {
                                skill_ids: Vec::new(),
                                turn_orchestration: Some(
                                    maka_runtime::message::TurnOrchestration {
                                        mode,
                                        source:
                                            maka_runtime::message::TurnOrchestrationSource::HostApi,
                                    },
                                ),
                            }
                        }),
                    };
                    let admitted_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_| invalid("system clock precedes epoch"))?
                        .as_millis();
                    let pending = PendingMessageAdmission {
                        invocation: receipt.invocation.clone(),
                        steering_invocation: None,
                        source,
                        required_tools: Default::default(),
                        admitted_at: u64::try_from(admitted_at)
                            .map_err(|_| invalid("clock overflow"))?,
                    };
                    insert::insert(&mut tx, &pending, insert::Owner::Unsealed).await?;
                    sqlx::query("INSERT INTO plugin_execution_receipts VALUES (?, ?, ?, ?)")
                        .bind(&package)
                        .bind(&scope)
                        .bind(&request.operation_id)
                        .bind(serde_json::to_string(&receipt)?)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_modify(|_| {});
                    Ok(receipt)
                })
            })
            .await
    }

    pub async fn plugin_execution_receipt(
        &self,
        namespace: &Namespace,
        operation_id: &str,
    ) -> Result<Option<Receipt>, StoreError> {
        self.validate_root()?;
        if operation_id.is_empty()
            || operation_id.len() > 256
            || operation_id.chars().any(char::is_control)
        {
            return Err(invalid("invalid plugin operation identity"));
        }
        let package = namespace.package().to_owned();
        let scope = String::from(namespace.scope().clone());
        let operation = operation_id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move { read(connection, &package, &scope, &operation).await })
            })
            .await
    }
}

async fn read(
    connection: &mut SqliteConnection,
    package: &str,
    scope: &str,
    operation: &str,
) -> Result<Option<Receipt>, StoreError> {
    let row: Option<Option<String>> = sqlx::query_scalar(
        "SELECT CASE WHEN length(CAST(receipt_json AS BLOB)) <= 4096 THEN receipt_json END
         FROM plugin_execution_receipts WHERE package_id = ? AND scope_id = ? AND operation_id = ?",
    )
    .bind(package)
    .bind(scope)
    .bind(operation)
    .fetch_optional(connection)
    .await?;
    row.map(|encoded| {
        let receipt: Receipt = serde_json::from_str(&encoded.ok_or(StoreError::PrefixTooLarge)?)?;
        for id in [
            &receipt.invocation.session_id,
            &receipt.invocation.turn_id,
            &receipt.invocation.run_id,
            &receipt.invocation.invocation_id,
            &receipt.message_id,
        ] {
            crate::sessions::validate_id(id)?;
        }
        if !maka_runtime::archive::valid_projection_digest(&receipt.content_digest) {
            return Err(invalid("invalid plugin execution receipt"));
        }
        Ok(receipt)
    })
    .transpose()
}
