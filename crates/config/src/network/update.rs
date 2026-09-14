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

use crate::{ConfigError, ConfigurationStore, Result, TransactionMode, catalog, policy, vault};
use maka_runtime::configuration::{
    policy::network_update::{self, CredentialTarget, CredentialUpdate, Update, UpdateResult},
    validation,
};
use sqlx::SqliteConnection;

impl ConfigurationStore {
    pub async fn update_network_proxy(&self, mut input: Update, now: u64) -> Result<UpdateResult> {
        input.normalize().map_err(ConfigError::Invalid)?;
        validation::revision(now, false).map_err(ConfigError::Invalid)?;
        self.transaction(TransactionMode::Immediate, move |tx| {
            Box::pin(async move {
                let mut snapshot = policy::read(tx).await?;
                if snapshot.revision != input.expected_policy_revision {
                    return Ok(UpdateResult::RevisionConflict {
                        expected_revision: input.expected_policy_revision,
                        actual_revision: snapshot.revision,
                    });
                }
                if let CredentialUpdate::Replace {
                    expected_target: Some(expected),
                    ..
                } = &input.credential
                {
                    let actual = CredentialTarget::from_proxy(&snapshot.policy.network_proxy);
                    if *expected != actual {
                        return Ok(UpdateResult::ProxyTargetMismatch {
                            expected: expected.clone(),
                            actual,
                        });
                    }
                }
                let locator = network_update::locator();
                let previous = vault::status(tx, &locator).await?;
                let actual = vault::status_basis(&previous);
                if input.expected_credential != actual {
                    return Ok(UpdateResult::CredentialStale {
                        expected: input.expected_credential,
                        actual,
                    });
                }
                let old: Option<String> =
                    sqlx::query_scalar("SELECT secret FROM credentials WHERE locator = ?")
                        .bind(serde_json::to_string(&locator)?)
                        .fetch_optional(&mut *tx)
                        .await?;
                let credential_changed = match &input.credential {
                    CredentialUpdate::Keep {} => false,
                    CredentialUpdate::Delete {} => old.is_some(),
                    CredentialUpdate::Replace { secret, .. } => old.as_ref() != Some(secret),
                };
                let policy_changed = snapshot.policy.network_proxy != input.network_proxy;
                if credential_changed
                    || !network_update::same_route(
                        &snapshot.policy.network_proxy,
                        &input.network_proxy,
                    )
                {
                    invalidate_tests(tx).await?;
                }
                if credential_changed {
                    match &input.credential {
                        CredentialUpdate::Replace { secret, .. } => {
                            vault::write_secret(tx, &locator, secret, now).await?
                        }
                        CredentialUpdate::Delete {} => {
                            sqlx::query("DELETE FROM credentials WHERE locator = ?")
                                .bind(serde_json::to_string(&locator)?)
                                .execute(&mut *tx)
                                .await?;
                        }
                        CredentialUpdate::Keep {} => unreachable!(),
                    }
                    vault::advance(tx).await?;
                }
                if policy_changed {
                    snapshot.revision = catalog::next_revision(snapshot.revision)?;
                    snapshot.policy.network_proxy = input.network_proxy;
                    policy::write(tx, &snapshot).await?;
                }
                Ok(UpdateResult::Committed {
                    revision: snapshot.revision,
                    credential_status: vault::status(tx, &locator).await?,
                })
            })
        })
        .await
    }
}

pub(crate) async fn invalidate_tests(tx: &mut SqliteConnection) -> Result<()> {
    let catalog = catalog::read(tx).await?;
    let mut changed = false;
    for mut row in catalog.connections {
        if row.last_test.is_none()
            || crate::model_catalog::provider_facts(&row.provider_type)?.retired
        {
            continue;
        }
        row.last_test = None;
        row.revision = catalog::next_revision(row.revision)?;
        catalog::write_entry(tx, &row).await?;
        changed = true;
    }
    if changed {
        catalog::advance(tx, catalog.revision, catalog.default_target.as_ref()).await?;
    }
    Ok(())
}
