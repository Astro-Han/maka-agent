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

use maka_runtime::configuration::*;
use sqlx::SqliteConnection;
mod headers;
mod write;
pub(crate) use write::write_secret;

use crate::{ConfigError, ConfigurationStore, Result, TransactionMode, catalog};

impl ConfigurationStore {
    pub async fn credential_status(
        &self,
        locator: CredentialLocator,
    ) -> Result<CredentialVaultQueryResult> {
        validation::locator(&locator).map_err(ConfigError::Invalid)?;
        self.transaction(TransactionMode::Deferred, move |tx| {
            Box::pin(async move {
                if let CredentialLocator::Connection { connection_id, .. } = &locator
                    && catalog::find(tx, connection_id).await?.is_none()
                {
                    return Ok(CredentialVaultQueryResult::ConnectionNotFound);
                }
                Ok(CredentialVaultQueryResult::Status {
                    status: status(tx, &locator).await?,
                })
            })
        })
        .await
    }

    pub async fn set_credential(
        &self,
        mut input: SetCredentialInput,
        now: u64,
    ) -> Result<SetCredentialResult> {
        validation::normalize_set_credential(&mut input).map_err(ConfigError::Invalid)?;
        validation::revision(now, false).map_err(ConfigError::Invalid)?;
        self.transaction(TransactionMode::Immediate, move |tx| {
            Box::pin(async move {
                if let Some(conflict) =
                    connection_conflict(tx, &input.locator, input.expected_connection.as_ref())
                        .await?
                {
                    return Ok(conflict);
                }
                let previous = status(tx, &input.locator).await?;
                if let CredentialLocator::Connection {
                    connection_id,
                    kind: ConnectionCredentialKind::OauthToken,
                } = &input.locator
                    && catalog::find(tx, connection_id)
                        .await?
                        .is_some_and(|row| row.provider_type != "github-copilot")
                {
                    return Err(ConfigError::Invalid(
                        "client OAuth credentials are only accepted for GitHub Copilot".into(),
                    ));
                }
                let actual = status_basis(&previous);
                let expected = input.expected.as_ref().map(|basis| CredentialVersionBasis {
                    locator: input.locator.clone(),
                    credential_id: basis.credential_id.clone(),
                    revision: basis.revision,
                });
                if expected != actual {
                    return Ok(CredentialMutationResult::CredentialStale { expected, actual });
                }
                write_secret(tx, &input.locator, &input.secret, now).await?;
                invalidate_test(tx, &input.locator).await?;
                Ok(CredentialMutationResult::Committed {
                    vault_revision: advance(tx).await?,
                    status: status(tx, &input.locator).await?,
                })
            })
        })
        .await
    }

    pub async fn delete_credential(
        &self,
        input: DeleteCredentialInput,
    ) -> Result<DeleteCredentialResult> {
        validation::credential_basis(&input.expected).map_err(ConfigError::Invalid)?;
        self.transaction(TransactionMode::Immediate, move |tx| {
            Box::pin(async move {
                if let Some(conflict) =
                    connection_conflict(tx, &input.expected.locator, None).await?
                {
                    return Ok(conflict);
                }
                let previous = status(tx, &input.expected.locator).await?;
                let actual = status_basis(&previous);
                if actual.as_ref() != Some(&input.expected) {
                    return Ok(CredentialMutationResult::CredentialStale {
                        expected: Some(input.expected),
                        actual,
                    });
                }
                sqlx::query("DELETE FROM credentials WHERE locator = ?")
                    .bind(serde_json::to_string(&input.expected.locator)?)
                    .execute(&mut *tx)
                    .await?;
                invalidate_test(tx, &input.expected.locator).await?;
                Ok(CredentialMutationResult::Committed {
                    vault_revision: advance(tx).await?,
                    status: status(tx, &input.expected.locator).await?,
                })
            })
        })
        .await
    }

    /// Trusted execution/export access only. Public status/catalog projections
    /// never call this or receive secret material.
    pub async fn credential_secret(
        &self,
        locator: &CredentialLocator,
        expected: Option<&ConnectionCredentialTarget>,
    ) -> Result<Option<String>> {
        let locator = locator.clone();
        let expected = expected.cloned();
        self.transaction(TransactionMode::Deferred, move |tx| {
            Box::pin(async move {
                if connection_conflict(tx, &locator, expected.as_ref())
                    .await?
                    .is_some()
                {
                    return Err(ConfigError::Invalid(
                        "credential connection basis changed".into(),
                    ));
                }
                sqlx::query_scalar("SELECT secret FROM credentials WHERE locator = ?")
                    .bind(serde_json::to_string(&locator)?)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(Into::into)
            })
        })
        .await
    }
}

pub(crate) async fn connection_conflict(
    tx: &mut SqliteConnection,
    locator: &CredentialLocator,
    expected: Option<&ConnectionCredentialTarget>,
) -> Result<Option<CredentialMutationResult>> {
    let CredentialLocator::Connection {
        connection_id,
        kind,
    } = locator
    else {
        return Ok(None);
    };
    let Some(row) = catalog::find(tx, connection_id).await? else {
        return Ok(Some(CredentialMutationResult::ConnectionNotFound));
    };
    if let Some(expected) = expected {
        let endpoint = row
            .base_url
            .as_deref()
            .or_else(|| validation::provider_default_base_url(&row.provider_type).ok())
            .unwrap_or("");
        let endpoint = validation::normalize_base_url(Some(endpoint), None)
            .map_err(ConfigError::Invalid)?
            .unwrap_or_default();
        let expected_endpoint =
            validation::normalize_base_url(Some(&expected.effective_base_url), None)
                .map_err(ConfigError::Invalid)?
                .unwrap_or_default();
        if expected.connection_id != row.connection_id
            || expected.revision != row.revision
            || expected.slug != row.slug
            || expected.provider_type != row.provider_type
            || expected_endpoint != endpoint
        {
            return Ok(Some(CredentialMutationResult::ConnectionStale {
                expected: ConnectionVersionBasis {
                    connection_id: expected.connection_id.clone(),
                    revision: expected.revision,
                },
                actual: Some(catalog::basis(&row)),
            }));
        }
    }
    let auth = validation::provider_auth_kind(&row.provider_type).map_err(ConfigError::Invalid)?;
    if (matches!(kind, ConnectionCredentialKind::ApiKey) && auth != ProviderAuthKind::ApiKey)
        || (matches!(kind, ConnectionCredentialKind::OauthToken)
            && auth != ProviderAuthKind::OauthToken)
    {
        return Err(ConfigError::Invalid(
            "credential kind does not match provider authentication".into(),
        ));
    }
    Ok(None)
}

pub(crate) async fn status(
    tx: &mut SqliteConnection,
    locator: &CredentialLocator,
) -> Result<CredentialStatus> {
    let saved: Option<(String, i64, i64)> = sqlx::query_as(
        "SELECT credential_id, revision, updated_at FROM credentials WHERE locator = ?",
    )
    .bind(serde_json::to_string(locator)?)
    .fetch_optional(&mut *tx)
    .await?;
    Ok(CredentialStatus {
        locator: locator.clone(),
        state: match saved {
            Some((credential_id, revision, updated_at)) => CredentialState::Configured {
                credential_id,
                revision: crate::database::unsigned(revision)?,
                updated_at: crate::database::unsigned(updated_at)?,
            },
            None => CredentialState::Absent,
        },
    })
}

pub(crate) fn status_basis(status: &CredentialStatus) -> Option<CredentialVersionBasis> {
    let CredentialState::Configured {
        credential_id,
        revision,
        ..
    } = &status.state
    else {
        return None;
    };
    Some(CredentialVersionBasis {
        locator: status.locator.clone(),
        credential_id: credential_id.clone(),
        revision: *revision,
    })
}

pub(crate) async fn advance(tx: &mut SqliteConnection) -> Result<u64> {
    let previous: i64 =
        sqlx::query_scalar("SELECT revision FROM credential_vault WHERE singleton = 1")
            .fetch_one(&mut *tx)
            .await?;
    let next = catalog::next_revision(crate::database::unsigned(previous)?)?;
    sqlx::query("UPDATE credential_vault SET revision = ? WHERE singleton = 1")
        .bind(next as i64)
        .execute(&mut *tx)
        .await?;
    Ok(next)
}

async fn invalidate_test(tx: &mut SqliteConnection, locator: &CredentialLocator) -> Result<()> {
    if matches!(locator, CredentialLocator::NetworkProxy { .. }) {
        return crate::network::invalidate_tests(tx).await;
    }
    if let CredentialLocator::Connection { connection_id, .. } = locator
        && let Some(mut row) = catalog::find(tx, connection_id).await?
        && row.last_test.is_some()
    {
        row.last_test = None;
        row.revision = catalog::next_revision(row.revision)?;
        catalog::write_entry(tx, &row).await?;
        let current = catalog::read(tx).await?;
        catalog::advance(tx, current.revision, current.default_target.as_ref()).await?;
    }
    Ok(())
}
