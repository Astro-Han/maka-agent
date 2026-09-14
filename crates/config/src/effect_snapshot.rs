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

use crate::{
    ConfigError, ConfigurationStore, Result, model_catalog, network::NetworkSnapshot,
    oauth::OAuthCredential, vault,
};
use maka_runtime::configuration::*;
use sqlx::SqliteConnection;
use std::sync::Arc;

/// A single SQL snapshot of the facts that determine a connection effect.
pub(crate) struct EffectSnapshot {
    pub connection: ConnectionCatalogEntry,
    pub endpoint: String,
    pub network: NetworkSnapshot,
    credential: SecretSnapshot,
    headers: SecretSnapshot,
}

struct SecretSnapshot {
    locator: CredentialLocator,
    basis: Option<CredentialVersionBasis>,
    secret: Option<String>,
}

impl EffectSnapshot {
    pub async fn read(
        tx: &mut SqliteConnection,
        connection: ConnectionCatalogEntry,
    ) -> Result<Self> {
        let id = &connection.connection_id;
        let kind = if model_catalog::provider_facts(&connection.provider_type)?.auth_kind
            == ProviderAuthKind::OauthToken
        {
            ConnectionCredentialKind::OauthToken
        } else {
            ConnectionCredentialKind::ApiKey
        };
        let credential = secret(tx, id, kind).await?;
        let headers = secret(tx, id, ConnectionCredentialKind::RequestHeaders).await?;
        Ok(Self {
            endpoint: endpoint(&connection)?,
            network: NetworkSnapshot::read(tx).await?,
            connection,
            credential,
            headers,
        })
    }

    pub fn api_key(&self) -> Option<&str> {
        match self.credential.locator {
            CredentialLocator::Connection {
                kind: ConnectionCredentialKind::ApiKey,
                ..
            } => self.credential.secret.as_deref(),
            _ => None,
        }
    }

    pub fn has_credential(&self) -> bool {
        self.credential.secret.is_some()
    }

    pub fn oauth_credential(&self, store: &Arc<ConfigurationStore>) -> Option<OAuthCredential> {
        if !matches!(
            self.credential.locator,
            CredentialLocator::Connection {
                kind: ConnectionCredentialKind::OauthToken,
                ..
            }
        ) {
            return None;
        }
        Some(OAuthCredential {
            store: store.clone(),
            basis: self.credential.basis.clone()?,
            secret: self.credential.secret.as_deref()?.into(),
            network: self.network.configuration.clone(),
            target: ConnectionCredentialTarget {
                connection_id: self.connection.connection_id.clone(),
                revision: self.connection.revision,
                provider_type: self.connection.provider_type.clone(),
                slug: self.connection.slug.clone(),
                effective_base_url: self.endpoint.clone(),
            },
        })
    }

    /// Pin the exact generation actually used by the authenticated request. Only
    /// the issuing root and the same credential identity can advance this basis.
    pub fn accept_oauth(
        &mut self,
        store: &Arc<ConfigurationStore>,
        resolved: OAuthCredential,
    ) -> Result<()> {
        let before = self
            .oauth_credential(store)
            .ok_or_else(|| ConfigError::Invalid("effect is not OAuth authenticated".into()))?;
        if !Arc::ptr_eq(&resolved.store, store)
            || resolved.basis.locator != before.basis.locator
            || resolved.basis.credential_id != before.basis.credential_id
            || resolved.basis.revision < before.basis.revision
            || resolved.target.provider_type != before.target.provider_type
            || resolved.target.slug != before.target.slug
            || resolved.target.effective_base_url != before.target.effective_base_url
        {
            return Err(ConfigError::Invalid(
                "OAuth observation changed credential identity".into(),
            ));
        }
        self.credential.basis = Some(resolved.basis);
        self.credential.secret = Some(resolved.secret.to_string());
        Ok(())
    }

    pub fn request_headers(&self) -> Option<&str> {
        self.headers.secret.as_deref()
    }

    pub async fn changed(
        &self,
        tx: &mut SqliteConnection,
        current: Option<&ConnectionCatalogEntry>,
    ) -> Result<Vec<ConnectionEffectChangedDomain>> {
        use ConnectionEffectChangedDomain as Changed;
        let mut changed = Vec::new();
        if current.is_none_or(|row| {
            !row.enabled
                || row.provider_type != self.connection.provider_type
                || row.enabled_model_ids != self.connection.enabled_model_ids
        }) || match current {
            Some(row) => endpoint(row)? != self.endpoint,
            None => true,
        } {
            changed.push(Changed::Connection);
        }
        if self.credentials_changed(tx).await? {
            changed.push(Changed::Credential);
        }
        if self.network_changed(tx).await? {
            changed.push(Changed::NetworkProxy);
        }
        Ok(changed)
    }

    pub async fn network_changed(&self, tx: &mut SqliteConnection) -> Result<bool> {
        self.network.changed(tx).await
    }

    pub async fn credentials_changed(&self, tx: &mut SqliteConnection) -> Result<bool> {
        for snapshot in [&self.credential, &self.headers] {
            let current = vault::status(tx, &snapshot.locator).await?;
            if vault::status_basis(&current) != snapshot.basis {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

async fn secret(
    tx: &mut SqliteConnection,
    id: &str,
    kind: ConnectionCredentialKind,
) -> Result<SecretSnapshot> {
    let locator = CredentialLocator::Connection {
        connection_id: id.into(),
        kind,
    };
    let status = vault::status(tx, &locator).await?;
    let secret = sqlx::query_scalar("SELECT secret FROM credentials WHERE locator = ?")
        .bind(serde_json::to_string(&locator)?)
        .fetch_optional(tx)
        .await?;
    Ok(SecretSnapshot {
        locator,
        basis: vault::status_basis(&status),
        secret,
    })
}

fn endpoint(row: &ConnectionCatalogEntry) -> Result<String> {
    let effective = row.base_url.as_deref().unwrap_or(
        validation::provider_default_base_url(&row.provider_type).map_err(ConfigError::Invalid)?,
    );
    validation::normalize_base_url(Some(effective), None)
        .map_err(ConfigError::Invalid)?
        .ok_or_else(|| ConfigError::Invalid("connection has no effective endpoint".into()))
}

pub(crate) fn same_test_basis(
    before: &ConnectionCatalogEntry,
    after: &ConnectionCatalogEntry,
) -> bool {
    before.enabled_model_ids == after.enabled_model_ids
        && before.model_source == after.model_source
        && test_models(before) == test_models(after)
}

fn test_models(
    row: &ConnectionCatalogEntry,
) -> std::collections::BTreeMap<String, Option<maka_runtime::configuration::ApiProtocol>> {
    let mut models: std::collections::BTreeMap<_, _> = row
        .enabled_model_ids
        .iter()
        .map(|id| (id.clone(), None))
        .collect();
    for model in row.effective_models() {
        models.insert(model.id, model.api_protocol);
    }
    models
}
