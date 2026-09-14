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
use maka_runtime::oauth::{ConnectionIdentity, LoginStart, Provider, Target};
pub use receipt::LoginReceipt;

pub enum LoginPreparation {
    Ready(Box<PreparedLogin>),
    Authenticated(LoginReceipt),
    Rejected(LoginRejection),
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoginRejection {
    AttemptConflict,
    ConnectionNotFound,
    ProviderUnavailable,
    CatalogFull,
    SlugTaken,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoginCompletion {
    Committed(LoginReceipt),
    Superseded { connection: bool, credential: bool },
    AttemptConflict,
    SlugTaken,
}

/// Only the originating store can issue this ticket. Consuming it commits at most
/// once. Provider authorization and entitlement must finish before completion.
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
        receipt::validate_attempt(&input.attempt_id)?;
        input
            .target
            .validate_create_identity()
            .map_err(ConfigError::Invalid)?;
        if let Target::Existing { connection_id } = &input.target {
            validation::entity_id(connection_id).map_err(ConfigError::Invalid)?;
        }
        let store = Arc::clone(self);
        self.transaction(TransactionMode::Deferred, move |tx| {
            Box::pin(async move {
                use LoginPreparation as P;
                use LoginRejection as R;
                if let Some(saved) = receipt::read(tx, &input.attempt_id).await? {
                    return Ok(if saved.target == input.target {
                        P::Authenticated(saved)
                    } else {
                        P::Rejected(R::AttemptConflict)
                    });
                }
                let catalog = catalog::read(tx).await?;
                let (before, mut after, provider) = match &input.target {
                    Target::Create {
                        provider_type,
                        slug,
                        name,
                    } => {
                        if slug.as_ref().is_some_and(|slug| {
                            catalog.connections.iter().any(|row| row.slug == *slug)
                        }) {
                            return Ok(P::Rejected(R::SlugTaken));
                        }
                        if catalog.connections.len() >= 1024 {
                            return Ok(P::Rejected(R::CatalogFull));
                        }
                        let mut row = new_connection(*provider_type, &catalog.connections)?;
                        if let Some(slug) = slug {
                            row.slug = slug.clone();
                        }
                        if let Some(name) = name {
                            row.name = name.clone();
                        }
                        (None, row, *provider_type)
                    }
                    Target::Existing { connection_id } => {
                        let Some(row) = catalog
                            .connections
                            .iter()
                            .find(|row| &row.connection_id == connection_id)
                            .cloned()
                        else {
                            return Ok(P::Rejected(R::ConnectionNotFound));
                        };
                        let provider = match row.provider_type.as_str() {
                            "openai-codex" => Provider::OpenaiCodex,
                            "github-copilot" => Provider::GithubCopilot,
                            "xai-oauth" => Provider::XaiOauth,
                            _ => return Ok(P::Rejected(R::ProviderUnavailable)),
                        };
                        (Some(row.clone()), row, provider)
                    }
                };
                if !after.enabled || after.last_test.is_some() {
                    after.revision = catalog::next_revision(after.revision)?;
                    after.enabled = true;
                    after.last_test = None;
                }
                let identity = ConnectionIdentity {
                    connection_id: after.connection_id.clone(),
                    slug: after.slug.clone(),
                    provider_type: provider,
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
        kind: ConnectionCredentialKind::OauthToken,
    }
}

fn new_connection(
    provider: Provider,
    rows: &[ConnectionCatalogEntry],
) -> Result<ConnectionCatalogEntry> {
    let facts = model_catalog::provider_facts(provider.as_str())?;
    let base = match provider {
        Provider::OpenaiCodex => "codex-subscription",
        Provider::GithubCopilot => "github-copilot",
        Provider::XaiOauth => "xai-oauth",
    };
    let slug = (1..)
        .map(|n| {
            if n == 1 {
                base.into()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|slug| rows.iter().all(|row| row.slug != *slug))
        .expect("bounded catalog");
    Ok(ConnectionCatalogEntry {
        connection_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
        slug,
        name: facts.label.clone(),
        provider_type: provider.as_str().into(),
        base_url: None,
        enabled: true,
        enabled_model_ids: facts.fallback_models.clone(),
        model_overrides: None,
        request_body_overlay: None,
        models: vec![],
        model_source: None,
        models_fetched_at: None,
        last_test: None,
    })
}
