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

use crate::{ConfigError, ConfigurationStore, Result, TransactionMode, catalog, vault};
use maka_runtime::configuration::*;

impl ConfigurationStore {
    pub async fn update_connection(
        &self,
        input: UpdateCatalogConnectionInput,
    ) -> Result<UpdateCatalogConnectionResult> {
        validation::basis(&input.expected).map_err(ConfigError::Invalid)?;
        self.transaction(TransactionMode::Immediate, move |tx| {
            Box::pin(async move {
                let current = catalog::read(tx).await?;
                let Some(mut row) = catalog::find(tx, &input.expected.connection_id).await? else {
                    return Ok(CatalogMutationResult::ConnectionStale {
                        expected: input.expected,
                        actual: None,
                    });
                };
                if row.revision != input.expected.revision {
                    return Ok(CatalogMutationResult::ConnectionStale {
                        expected: input.expected,
                        actual: Some(catalog::basis(&row)),
                    });
                }
                if row.provider_type == "gemini-cli" {
                    return Err(ConfigError::Invalid(
                        "retired provider cannot be edited".into(),
                    ));
                }
                let changes =
                    validation::normalize_update_for_provider(input.changes, &row.provider_type)
                        .map_err(ConfigError::Invalid)?;
                let endpoint_changed = row.base_url != changes.base_url;
                let overlay_changed = match &changes.request_body_overlay {
                    Patch::Keep => false,
                    Patch::Clear => row.request_body_overlay.is_some(),
                    Patch::Set(value) => row.request_body_overlay.as_ref() != Some(value),
                };
                let previous = row.clone();
                let validate_limits = matches!(changes.model_overrides, Patch::Set(_));
                let mut test_changed = endpoint_changed
                    || row.enabled != changes.enabled
                    || row.enabled_model_ids != changes.enabled_model_ids
                    || overlay_changed;
                row.revision = catalog::next_revision(row.revision)?;
                row.name = changes.name;
                row.base_url = changes.base_url;
                row.enabled = changes.enabled;
                row.enabled_model_ids = changes.enabled_model_ids;
                row.model_overrides = match changes.model_overrides {
                    Patch::Set(profiles) => Some(profiles),
                    Patch::Clear => None,
                    Patch::Keep if endpoint_changed => None,
                    Patch::Keep => row.model_overrides,
                };
                row.request_body_overlay = match changes.request_body_overlay {
                    Patch::Keep => row.request_body_overlay,
                    Patch::Clear => None,
                    Patch::Set(value) => Some(value),
                };
                if endpoint_changed {
                    row.models.clear();
                    row.model_source = None;
                    row.models_fetched_at = None;
                }
                test_changed |= !crate::effect_snapshot::same_test_basis(&previous, &row);
                if validate_limits {
                    crate::model_catalog::validate_overrides(&row)?;
                }
                if test_changed {
                    row.last_test = None;
                }
                catalog::write_entry(tx, &row).await?;
                let rows = catalog::read(tx).await?.connections;
                let target = current
                    .default_target
                    .filter(|target| catalog::valid_target(target, &rows));
                Ok(CatalogMutationResult::Committed {
                    catalog_revision: catalog::advance(tx, current.revision, target.as_ref())
                        .await?,
                    connection: Some(catalog::basis(&row)),
                })
            })
        })
        .await
    }

    /// Catalog deletion and credential cleanup share one durable transaction.
    /// Repeating a removal of an absent connection is a successful no-op.
    pub async fn remove_connection(
        &self,
        input: RemoveCatalogConnectionInput,
    ) -> Result<RemoveCatalogConnectionResult> {
        validation::basis(&input.expected).map_err(ConfigError::Invalid)?;
        self.transaction(TransactionMode::Immediate, move |tx| {
            Box::pin(async move {
                let current = catalog::read(tx).await?;
                let existing = catalog::find(tx, &input.expected.connection_id).await?;
                if let Some(row) = &existing
                    && row.revision != input.expected.revision
                {
                    return Ok(CatalogMutationResult::ConnectionStale {
                        expected: input.expected,
                        actual: Some(catalog::basis(row)),
                    });
                }
                let removed = sqlx::query(
                    "DELETE FROM credentials WHERE json_extract(locator, '$.scope') = 'connection'
                    AND json_extract(locator, '$.connectionId') = ?",
                )
                .bind(&input.expected.connection_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if removed > 0 {
                    vault::advance(tx).await?;
                }
                let revision = if existing.is_some() {
                    sqlx::query("DELETE FROM connections WHERE connection_id = ?")
                        .bind(&input.expected.connection_id)
                        .execute(&mut *tx)
                        .await?;
                    let target = current
                        .default_target
                        .filter(|target| target.connection_id != input.expected.connection_id);
                    catalog::advance(tx, current.revision, target.as_ref()).await?
                } else {
                    current.revision
                };
                Ok(CatalogMutationResult::Committed {
                    catalog_revision: revision,
                    connection: None,
                })
            })
        })
        .await
    }
}
