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

use super::*;

impl PreparedLogin {
    /// Known rejection/cancellation is terminal, not an unresolved spent grant.
    pub async fn finish_failure(&self, phase: Phase) -> Result<Phase> {
        if !matches!(phase, Phase::Cancelled | Phase::Failed { .. })
            || phase
                == (Phase::Failed {
                    failure: maka_runtime::oauth::Failure::OutcomeUnknown,
                })
        {
            return Err(ConfigError::Invalid("not an authentication failure".into()));
        }
        let ticket = self.clone();
        self.store
            .transaction(TransactionMode::Immediate, move |tx| {
                Box::pin(async move {
                    let Some(mut saved) = receipt::read(tx, &ticket.input.attempt_id).await? else {
                        return Err(ConfigError::Invalid(
                            "authentication was not claimed".into(),
                        ));
                    };
                    if !saved.matches(&ticket.input) || saved.connection != ticket.identity {
                        return Err(ConfigError::Invalid("authentication claim changed".into()));
                    }
                    if saved.phase == Phase::Exchanging {
                        saved.phase = phase;
                        receipt::write(tx, &ticket.input.attempt_id, &saved).await?;
                    }
                    Ok(saved.phase)
                })
            })
            .await
    }

    /// Persist before running the already-admitted authentication callback.
    /// An existing claim never authorizes a second exchange, including restart.
    pub async fn claim(&self) -> Result<bool> {
        let ticket = self.clone();
        self.store
            .transaction(TransactionMode::Immediate, move |tx| {
                Box::pin(async move {
                    if receipt::read(tx, &ticket.input.attempt_id).await?.is_some() {
                        return Ok(false);
                    }
                    if receipt::pending_count(tx).await? >= 256 {
                        return Err(ConfigError::Invalid(
                            "unresolved login capacity is exhausted".into(),
                        ));
                    }
                    let attempt = ticket.input.attempt_id.clone();
                    let mut receipt = LoginReceipt::new(ticket.input, ticket.identity)?;
                    receipt.phase = Phase::Exchanging;
                    receipt::write(tx, &attempt, &receipt).await?;
                    Ok(true)
                })
            })
            .await
    }

    /// Atomically publish the connection, its credential and completion receipt.
    /// Once queued, dropping the waiter does not abandon the database operation.
    /// CommitUnknown is not authentication success: reconcile the durable receipt.
    pub async fn complete(&self, replacement: Credential, now: u64) -> Result<LoginCompletion> {
        replacement.validate().map_err(ConfigError::Invalid)?;
        let secret = serde_json::to_string(&replacement)?;
        validation::revision(now, false).map_err(ConfigError::Invalid)?;
        let Self {
            store,
            input,
            before,
            after,
            identity,
            credential,
            network: _,
        } = self.clone();
        store
            .transaction(TransactionMode::Immediate, move |tx| {
                Box::pin(async move {
                    if let Some(saved) = receipt::read(tx, &input.attempt_id).await? {
                        if !saved.matches(&input) || saved.connection != identity {
                            return Ok(LoginCompletion::AttemptConflict);
                        }
                        if saved.phase == Phase::Authenticated {
                            return Ok(LoginCompletion::Committed(Box::new(saved)));
                        }
                        match saved.phase {
                            Phase::Failed {
                                failure: maka_runtime::oauth::Failure::ConnectionChanged,
                            } => return Ok(LoginCompletion::ConnectionChanged),
                            Phase::Failed {
                                failure: maka_runtime::oauth::Failure::CredentialChanged,
                            } => return Ok(LoginCompletion::CredentialChanged),
                            Phase::Failed {
                                failure: maka_runtime::oauth::Failure::SlugTaken,
                            } => return Ok(LoginCompletion::SlugTaken),
                            Phase::Exchanging => {}
                            _ => return Ok(LoginCompletion::AttemptConflict),
                        }
                    }
                    let catalog = catalog::read(tx).await?;
                    if matches!(&input.target, Target::Create { .. })
                        && catalog.connections.iter().any(|row| {
                            row.slug == after.slug && row.connection_id != after.connection_id
                        })
                    {
                        return reject(
                            tx,
                            input,
                            identity,
                            maka_runtime::oauth::Failure::SlugTaken,
                        )
                        .await;
                    }
                    let actual = catalog
                        .connections
                        .iter()
                        .find(|row| row.connection_id == after.connection_id);
                    let connection_changed = match &before {
                        Some(before) => actual != Some(before),
                        None => {
                            actual.is_some()
                                || catalog.connections.len() >= 1024
                                || catalog.connections.iter().any(|row| row.slug == after.slug)
                        }
                    };
                    let locator = locator(&after);
                    let credential_changed =
                        vault::status_basis(&vault::status(tx, &locator).await?) != credential;
                    if connection_changed || credential_changed {
                        return reject(
                            tx,
                            input,
                            identity,
                            if connection_changed {
                                maka_runtime::oauth::Failure::ConnectionChanged
                            } else {
                                maka_runtime::oauth::Failure::CredentialChanged
                            },
                        )
                        .await;
                    }
                    if before.as_ref() != Some(&after) {
                        catalog::write_entry(tx, &after).await?;
                        catalog::advance(tx, catalog.revision, catalog.default_target.as_ref())
                            .await?;
                    }
                    vault::replace_secret(tx, &locator, &secret, now).await?;
                    vault::advance(tx).await?;
                    let attempt_id = input.attempt_id.clone();
                    let saved = LoginReceipt::new(input, identity)?;
                    receipt::write(tx, &attempt_id, &saved).await?;
                    Ok(LoginCompletion::Committed(Box::new(saved)))
                })
            })
            .await
    }
}

async fn reject(
    tx: &mut sqlx::SqliteConnection,
    input: LoginStart,
    identity: ConnectionIdentity,
    failure: maka_runtime::oauth::Failure,
) -> Result<LoginCompletion> {
    use maka_runtime::oauth::Failure;
    let completion = match failure {
        Failure::ConnectionChanged => LoginCompletion::ConnectionChanged,
        Failure::CredentialChanged => LoginCompletion::CredentialChanged,
        Failure::SlugTaken => LoginCompletion::SlugTaken,
        _ => return Err(ConfigError::Invalid("invalid login conflict".into())),
    };
    let attempt = input.attempt_id.clone();
    let mut receipt = LoginReceipt::new(input, identity)?;
    receipt.phase = Phase::Failed { failure };
    receipt::write(tx, &attempt, &receipt).await?;
    Ok(completion)
}
