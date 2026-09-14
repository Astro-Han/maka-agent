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
    ConfigError, ConfigurationStore, Result, TransactionMode, catalog, model_catalog,
    network::{NetworkConfiguration, NetworkSnapshot},
    vault,
};
use maka_runtime::configuration::*;
use std::sync::Arc;

pub mod enrollment;

/// An immutable, root-bound OAuth generation. Secret material never enters a
/// public projection. Clones share ownership, not an independent credential cache.
#[derive(Clone)]
pub struct OAuthCredential {
    pub(crate) store: Arc<ConfigurationStore>,
    pub(crate) target: ConnectionCredentialTarget,
    pub(crate) basis: CredentialVersionBasis,
    pub(crate) secret: Arc<str>,
    pub(crate) network: NetworkConfiguration,
}

impl ConfigurationStore {
    /// Pin credential identity, raw material and refresh routing in one SQL
    /// snapshot. None means the requested execution binding is no longer usable.
    pub async fn oauth_credential(
        self: &Arc<Self>,
        target: ConnectionCredentialTarget,
    ) -> Result<Option<OAuthCredential>> {
        validation::credential_target(&target).map_err(ConfigError::Invalid)?;
        let facts = model_catalog::provider_facts(&target.provider_type)?;
        if facts.retired || facts.auth_kind != ProviderAuthKind::OauthToken {
            return Err(ConfigError::Invalid(
                "provider does not support OAuth execution".into(),
            ));
        }
        let store = Arc::clone(self);
        self.transaction(TransactionMode::Deferred, move |tx| {
            Box::pin(async move {
                let locator = CredentialLocator::Connection {
                    connection_id: target.connection_id.clone(),
                    kind: ConnectionCredentialKind::OauthToken,
                };
                if vault::connection_conflict(tx, &locator, Some(&target))
                    .await?
                    .is_some()
                    || catalog::find(tx, &target.connection_id)
                        .await?
                        .is_none_or(|row| !row.enabled)
                {
                    return Ok(None);
                }
                let Some(basis) = vault::status_basis(&vault::status(tx, &locator).await?) else {
                    return Ok(None);
                };
                let secret: String =
                    sqlx::query_scalar("SELECT secret FROM credentials WHERE locator = ?")
                        .bind(serde_json::to_string(&locator)?)
                        .fetch_one(&mut *tx)
                        .await?;
                Ok(Some(OAuthCredential {
                    store,
                    target,
                    basis,
                    secret: secret.into(),
                    network: NetworkSnapshot::read(tx).await?.configuration,
                }))
            })
        })
        .await
    }
}

impl OAuthCredential {
    /// Reconcile this credential identity without treating metadata edits or a
    /// disabled connection as revocation of an already-spent refresh grant.
    /// A replacement login, logout or removed connection returns None.
    pub async fn current_generation(&self) -> Result<Option<Self>> {
        let snapshot = self.clone();
        self.store
            .transaction(TransactionMode::Deferred, move |tx| {
                Box::pin(async move {
                    let Some(row) = catalog::find(tx, &snapshot.target.connection_id).await? else {
                        return Ok(None);
                    };
                    let Some(basis) =
                        vault::status_basis(&vault::status(tx, &snapshot.basis.locator).await?)
                    else {
                        return Ok(None);
                    };
                    if row.provider_type != snapshot.target.provider_type
                        || row.slug != snapshot.target.slug
                        || basis.credential_id != snapshot.basis.credential_id
                    {
                        return Ok(None);
                    }
                    let secret: String =
                        sqlx::query_scalar("SELECT secret FROM credentials WHERE locator = ?")
                            .bind(serde_json::to_string(&basis.locator)?)
                            .fetch_one(&mut *tx)
                            .await?;
                    Ok(Some(Self {
                        basis,
                        secret: secret.into(),
                        ..snapshot
                    }))
                })
            })
            .await
    }

    pub fn target(&self) -> &ConnectionCredentialTarget {
        &self.target
    }

    pub fn basis(&self) -> &CredentialVersionBasis {
        &self.basis
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    pub fn network_configuration(&self) -> &NetworkConfiguration {
        &self.network
    }

    /// Commit only against this connection identity and credential generation.
    /// Metadata changes and disabling execution do not revoke a spent grant;
    /// logout, re-login and connection removal supersede its generation.
    /// A caller must retain a received replacement until this settles: cancellation
    /// of its waiter does not undo an accepted database job or a spent refresh grant.
    ///
    /// None is supersession (including logout); it never creates a credential.
    /// CommitUnknown remains unknown. Re-read the canonical snapshot and compare
    /// credential ID, next revision and exact replacement before using the result;
    /// do not refresh again with the old grant to resolve a persistence ambiguity.
    pub async fn commit_refresh(
        &self,
        secret: String,
        now: u64,
    ) -> Result<Option<CredentialVersionBasis>> {
        let input = SetCredentialInput {
            locator: self.basis.locator.clone(),
            expected: Some(CredentialIdentityBasis {
                credential_id: self.basis.credential_id.clone(),
                revision: self.basis.revision,
            }),
            expected_connection: Some(self.target.clone()),
            secret,
        };
        validation::validate_set_credential(&input).map_err(ConfigError::Invalid)?;
        validation::revision(now, false).map_err(ConfigError::Invalid)?;
        let expected = self.basis.clone();
        let target = self.target.clone();
        self.store
            .transaction(TransactionMode::Immediate, move |tx| {
                Box::pin(async move {
                    let Some(row) = catalog::find(tx, &target.connection_id).await? else {
                        return Ok(None);
                    };
                    if row.provider_type != target.provider_type
                        || row.slug != target.slug
                        || vault::status_basis(&vault::status(tx, &input.locator).await?).as_ref()
                            != Some(&expected)
                    {
                        return Ok(None);
                    }
                    // Provider token rotation does not change the connection's user
                    // configuration or invalidate its verification, unlike re-login.
                    vault::write_secret(tx, &input.locator, &input.secret, now).await?;
                    vault::advance(tx).await?;
                    Ok(vault::status_basis(
                        &vault::status(tx, &input.locator).await?,
                    ))
                })
            })
            .await
    }
}
