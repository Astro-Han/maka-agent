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

mod commit;
mod receipt;

use super::*;
use maka_runtime::oauth::{ConnectionIdentity, LoginStart, Phase, Target};
pub use receipt::LoginReceipt;

pub enum LoginPreparation {
    Ready(Box<PreparedLogin>),
    Finished(Box<LoginReceipt>),
    OutcomeUnknown(Box<LoginReceipt>),
    Rejected(LoginRejection),
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoginRejection {
    AttemptConflict,
    ConnectionNotFound,
    ConnectionChanged,
    CatalogFull,
    SlugTaken,
    AttemptsFull,
}

#[derive(Debug, PartialEq)]
pub enum LoginCompletion {
    Committed(Box<LoginReceipt>),
    ConnectionChanged,
    CredentialChanged,
    AttemptConflict,
    SlugTaken,
}

/// Store-issued ticket. Persistence may be retried with the received replacement;
/// neither a lost reply nor an uncertain commit authorizes a second exchange.
#[derive(Clone)]
pub struct PreparedLogin {
    store: Arc<ConfigurationStore>,
    input: LoginStart,
    before: Option<ConnectionCatalogEntry>,
    after: ConnectionCatalogEntry,
    identity: ConnectionIdentity,
    credential: Option<CredentialVersionBasis>,
    network: NetworkConfiguration,
}

impl ConfigurationStore {
    pub async fn prepare_oauth_login(
        self: &Arc<Self>,
        input: LoginStart,
    ) -> Result<LoginPreparation> {
        input.validate().map_err(ConfigError::Invalid)?;
        let store = Arc::clone(self);
        self.transaction(TransactionMode::Deferred, move |tx| {
            Box::pin(async move {
                use LoginPreparation as P;
                use LoginRejection as R;
                if let Some(saved) = receipt::read(tx, &input.attempt_id).await? {
                    return Ok(if saved.matches(&input) {
                        match saved.phase {
                            Phase::Exchanging => P::OutcomeUnknown(Box::new(saved)),
                            _ => P::Finished(Box::new(saved)),
                        }
                    } else {
                        P::Rejected(R::AttemptConflict)
                    });
                }
                if receipt::pending_count(tx).await? >= 256 {
                    return Ok(P::Rejected(R::AttemptsFull));
                }
                let catalog = catalog::read(tx).await?;
                let (before, mut after) = match &input.target {
                    Target::Create {
                        provider,
                        configuration,
                        slug,
                        name,
                    } => {
                        if catalog.connections.iter().any(|row| row.slug == *slug) {
                            return Ok(P::Rejected(R::SlugTaken));
                        }
                        if catalog.connections.len() >= 1024 {
                            return Ok(P::Rejected(R::CatalogFull));
                        }
                        (
                            None,
                            ConnectionCatalogEntry {
                                connection_id: uuid::Uuid::new_v4().to_string(),
                                revision: 1,
                                slug: slug.clone(),
                                name: name.clone(),
                                provider: provider.clone(),
                                configuration: configuration.clone(),
                                enabled: true,
                                enabled_model_ids: vec![],
                                model_overrides: None,
                                request_body_overlay: None,
                                models: vec![],
                                model_source: None,
                                models_fetched_at: None,
                                last_test: None,
                            },
                        )
                    }
                    Target::Existing {
                        expected,
                        configuration,
                    } => {
                        let Some(row) = catalog
                            .connections
                            .iter()
                            .find(|row| row.connection_id == expected.connection_id)
                            .cloned()
                        else {
                            return Ok(P::Rejected(R::ConnectionNotFound));
                        };
                        if vault::connection_conflict(tx, &locator(&row), Some(expected))
                            .await?
                            .is_some()
                        {
                            return Ok(P::Rejected(R::ConnectionChanged));
                        }
                        let before = row.clone();
                        let mut row = row;
                        if row.configuration != *configuration {
                            row.configuration = configuration.clone();
                            row.models.clear();
                            row.model_source = None;
                            row.models_fetched_at = None;
                            row.model_overrides = None;
                        }
                        (Some(before), row)
                    }
                };
                if before.is_some() {
                    // Every reauthentication invalidates verification, even if
                    // the provider returns the same secret bytes.
                    after.revision = catalog::next_revision(after.revision)?;
                    after.enabled = true;
                    after.last_test = None;
                }
                let identity = ConnectionIdentity {
                    connection_id: after.connection_id.clone(),
                    slug: after.slug.clone(),
                    provider: after.provider.clone(),
                };
                let credential = vault::status_basis(&vault::status(tx, &locator(&after)).await?);
                let network = NetworkSnapshot::read(tx).await?.configuration;
                Ok(P::Ready(Box::new(PreparedLogin {
                    store,
                    input,
                    before,
                    after,
                    identity,
                    credential,
                    network,
                })))
            })
        })
        .await
    }

    pub async fn oauth_login_receipt(&self, attempt_id: String) -> Result<Option<LoginReceipt>> {
        receipt::validate_attempt(&attempt_id)?;
        self.transaction(TransactionMode::Deferred, move |tx| {
            Box::pin(async move { receipt::read(tx, &attempt_id).await })
        })
        .await
    }
}

impl PreparedLogin {
    /// Defaults are materialized by the pinned provider exactly once. Keep the
    /// caller's original request unchanged so retries still match its receipt.
    pub fn configure_creation(&mut self, configuration: serde_json::Value) -> Result<()> {
        if self.before.is_some() {
            return Err(ConfigError::Invalid(
                "existing connections do not acquire new defaults".into(),
            ));
        }
        validation::provider_configuration(&configuration).map_err(ConfigError::Invalid)?;
        self.after.configuration = configuration;
        Ok(())
    }

    pub fn identity(&self) -> &ConnectionIdentity {
        &self.identity
    }
    pub fn connection(&self) -> &ConnectionCatalogEntry {
        &self.after
    }
    pub fn network_configuration(&self) -> &NetworkConfiguration {
        &self.network
    }
}

fn locator(row: &ConnectionCatalogEntry) -> CredentialLocator {
    CredentialLocator::Connection {
        connection_id: row.connection_id.clone(),
        kind: ConnectionCredentialKind::Provider,
    }
}
