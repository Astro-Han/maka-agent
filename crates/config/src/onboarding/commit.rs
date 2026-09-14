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
use crate::{effect_snapshot::same_test_basis, vault};

impl PreparedOnboarding {
    pub async fn complete(
        self,
        models: Vec<ModelInfo>,
        mut enabled: Vec<String>,
        now: u64,
    ) -> Result<OnboardingSaveResult> {
        validation::revision(now, false).map_err(ConfigError::Invalid)?;
        if models.is_empty() || models.len() > 2048 {
            return Ok(OnboardingSaveResult::Rejected {
                reason: OnboardingRejection::ModelUnavailable,
            });
        }
        let mut available = std::collections::HashSet::new();
        for model in &models {
            model.validate().map_err(ConfigError::Invalid)?;
            if !available.insert(model.id.clone()) {
                return Err(ConfigError::Invalid("duplicate discovered model".into()));
            }
        }
        if enabled.is_empty() {
            enabled = models.iter().map(|model| model.id.clone()).collect();
        }
        if enabled.iter().any(|id| !available.contains(id)) {
            return Ok(OnboardingSaveResult::Rejected {
                reason: OnboardingRejection::ModelUnavailable,
            });
        }
        validation::model_ids(&enabled).map_err(ConfigError::Invalid)?;
        let Self {
            store,
            material,
            original_revision,
            requested_slug,
            supplied_secret,
            protocol: _,
        } = self;
        store
            .transaction(TransactionMode::Immediate, move |tx| {
                Box::pin(async move {
                    use OnboardingRejection as R;
                    let reject = |reason| OnboardingSaveResult::Rejected { reason };
                    let catalog = catalog::read(tx).await?;
                    let mut row = material.connection.clone();
                    let previous = catalog
                        .connections
                        .iter()
                        .find(|r| r.connection_id == row.connection_id);
                    if let Some(revision) = original_revision {
                        let Some(previous) = previous else {
                            return Ok(reject(R::ConnectionNotFound));
                        };
                        if previous.revision != revision {
                            return Ok(reject(R::Superseded));
                        }
                    } else {
                        if catalog.connections.iter().any(|r| r.slug == row.slug) {
                            return Ok(reject(if requested_slug {
                                R::SlugTaken
                            } else {
                                R::Superseded
                            }));
                        }
                        if previous.is_some() {
                            return Ok(reject(R::Superseded));
                        }
                        if catalog.connections.len() >= 1024 {
                            return Ok(reject(R::CatalogFull));
                        }
                    }
                    if material.credentials_changed(tx).await?
                        || material.network_changed(tx).await?
                    {
                        return Ok(reject(R::Superseded));
                    }
                    // Keep manually enabled IDs that the discovery endpoint did not offer.
                    if let Some(previous) = previous {
                        for id in &previous.enabled_model_ids {
                            if !available.contains(id) && !enabled.contains(id) {
                                enabled.push(id.clone());
                            }
                        }
                        row.revision = catalog::next_revision(previous.revision)?;
                        if row.base_url != previous.base_url {
                            row.model_overrides = None;
                        }
                    }
                    let secret_changed = supplied_secret
                        .as_deref()
                        .is_some_and(|secret| Some(secret) != material.api_key());
                    row.enabled = true;
                    row.enabled_model_ids = enabled;
                    row.models = models;
                    row.model_source = Some(ModelDiscoverySource::Fetched);
                    row.models_fetched_at = Some(now);
                    if previous
                        .is_none_or(|p| p.base_url != row.base_url || !same_test_basis(p, &row))
                        || secret_changed
                    {
                        row.last_test = None;
                    }
                    catalog::write_entry(tx, &row).await?;
                    if secret_changed {
                        let locator = CredentialLocator::Connection {
                            connection_id: row.connection_id.clone(),
                            kind: ConnectionCredentialKind::ApiKey,
                        };
                        vault::write_secret(tx, &locator, supplied_secret.as_deref().unwrap(), now)
                            .await?;
                        vault::advance(tx).await?;
                    }
                    let target = catalog.default_target.unwrap_or_else(|| ConnectionTarget {
                        connection_id: row.connection_id.clone(),
                        model_id: row.enabled_model_ids[0].clone(),
                    });
                    catalog::advance(tx, catalog.revision, Some(&target)).await?;
                    Ok(OnboardingSaveResult::Saved {
                        connection: OnboardedConnection {
                            connection_id: row.connection_id,
                            revision: row.revision,
                            slug: row.slug,
                            provider_type: row.provider_type,
                        },
                    })
                })
            })
            .await
    }
}
