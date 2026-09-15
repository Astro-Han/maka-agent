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
    /// Atomically publish the connection, its credential and completion receipt.
    /// Once queued, dropping the waiter does not abandon the database operation.
    /// CommitUnknown is not authentication success: reconcile the durable receipt.
    pub async fn complete(self, secret: String, now: u64) -> Result<LoginCompletion> {
        validation::text(&secret, 64 * 1024, true).map_err(ConfigError::Invalid)?;
        validation::revision(now, false).map_err(ConfigError::Invalid)?;
        let Self {
            store,
            input,
            before,
            after,
            identity,
            credential,
            network: _,
        } = self;
        store
            .transaction(TransactionMode::Immediate, move |tx| {
                Box::pin(async move {
                    if receipt::read(tx, &input.attempt_id).await?.is_some() {
                        return Ok(LoginCompletion::AttemptConflict);
                    }
                    let catalog = catalog::read(tx).await?;
                    if matches!(&input.target, Target::Create { slug: Some(_), .. })
                        && catalog.connections.iter().any(|row| {
                            row.slug == after.slug && row.connection_id != after.connection_id
                        })
                    {
                        return Ok(LoginCompletion::SlugTaken);
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
                        return Ok(LoginCompletion::Superseded {
                            connection: connection_changed,
                            credential: credential_changed,
                        });
                    }
                    if before.as_ref() != Some(&after) {
                        catalog::write_entry(tx, &after).await?;
                        catalog::advance(tx, catalog.revision, catalog.default_target.as_ref())
                            .await?;
                    }
                    vault::replace_secret(tx, &locator, &secret, now).await?;
                    vault::advance(tx).await?;
                    let saved = LoginReceipt {
                        target: input.target,
                        connection: identity,
                    };
                    receipt::write(tx, &input.attempt_id, &saved).await?;
                    Ok(LoginCompletion::Committed(saved))
                })
            })
            .await
    }
}
