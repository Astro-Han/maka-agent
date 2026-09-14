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

//! One-shot discovery preparation and atomic connection/credential publication.
mod commit;
use crate::{
    ConfigError, ConfigurationStore, Result, TransactionMode, catalog,
    effect_snapshot::EffectSnapshot, model_catalog,
};
use maka_runtime::configuration::{onboarding::*, *};
use std::sync::Arc;
use uuid::Uuid;

pub enum OnboardingPreparation {
    Ready(Box<PreparedOnboarding>),
    Rejected(OnboardingRejection),
    Unsupported,
}

/// Store-bound ownership makes a ticket unforgeable and completion consumable once.
pub struct PreparedOnboarding {
    protocol: ModelListProtocol,
    store: Arc<ConfigurationStore>,
    material: EffectSnapshot,
    original_revision: Option<u64>,
    requested_slug: bool,
    supplied_secret: Option<String>,
}

impl ConfigurationStore {
    pub async fn prepare_onboarding(
        self: &Arc<Self>,
        input: OnboardingInput,
    ) -> Result<OnboardingPreparation> {
        let store = Arc::clone(self);
        self.transaction(TransactionMode::Deferred, move |tx| {
            Box::pin(async move {
                use OnboardingPreparation as P;
                use OnboardingRejection as R;
                let catalog = catalog::read(tx).await?;
                let (mut row, original_revision, requested_slug) = match input.target {
                    OnboardingTarget::Existing { connection_id } => {
                        let Some(row) = catalog
                            .connections
                            .iter()
                            .find(|r| r.connection_id == connection_id)
                            .cloned()
                        else {
                            return Ok(P::Rejected(R::ConnectionNotFound));
                        };
                        let revision = row.revision;
                        (row, Some(revision), false)
                    }
                    OnboardingTarget::Create {
                        provider_type,
                        slug,
                        name,
                    } => {
                        let facts = model_catalog::provider_facts(&provider_type)?;
                        if facts.retired || facts.auth_kind != ProviderAuthKind::ApiKey {
                            return Ok(P::Rejected(R::ProviderUnsupported));
                        }
                        if catalog.connections.len() >= 1024 {
                            return Ok(P::Rejected(R::CatalogFull));
                        }
                        let requested_slug = slug.is_some();
                        let slug = match slug {
                            Some(slug) => {
                                validation::slug(&slug).map_err(ConfigError::Invalid)?;
                                if catalog.connections.iter().any(|r| r.slug == slug) {
                                    return Ok(P::Rejected(R::SlugTaken));
                                }
                                slug
                            }
                            None => (1..)
                                .map(|n| {
                                    if n == 1 {
                                        provider_type.clone()
                                    } else {
                                        format!("{provider_type}-{n}")
                                    }
                                })
                                .find(|slug| catalog.connections.iter().all(|r| r.slug != *slug))
                                .expect("finite catalog"),
                        };
                        let name = name.unwrap_or_else(|| facts.label.clone());
                        validation::text(&name, 128, false).map_err(ConfigError::Invalid)?;
                        (
                            ConnectionCatalogEntry {
                                connection_id: Uuid::new_v4().to_string(),
                                revision: 1,
                                slug,
                                name,
                                provider_type,
                                base_url: None,
                                enabled: true,
                                enabled_model_ids: vec![],
                                model_overrides: Default::default(),
                                request_body_overlay: None,
                                models: vec![],
                                model_source: None,
                                models_fetched_at: None,
                                last_test: None,
                            },
                            None,
                            requested_slug,
                        )
                    }
                };
                let facts = model_catalog::provider_facts(&row.provider_type)?;
                if facts.retired || facts.auth_kind != ProviderAuthKind::ApiKey {
                    return Ok(P::Rejected(R::ProviderUnsupported));
                }
                let Some(protocol) = facts.native_model_list() else {
                    return Ok(P::Unsupported);
                };
                let override_url = validation::normalize_base_url(
                    input.base_url.as_deref(),
                    Some(&row.provider_type),
                )
                .map_err(ConfigError::Invalid)?;
                // Pin the existing credential/header statuses, but discover using the requested endpoint.
                row.base_url = override_url.or(row.base_url);
                if row.base_url.is_none() && facts.base_url.is_empty() {
                    return Ok(P::Rejected(R::BaseUrlNotConfigured));
                }
                let material = EffectSnapshot::read(tx, row).await?;
                let supplied_secret = input
                    .api_key
                    .map(|s| s.trim_matches(js_whitespace).to_owned())
                    .filter(|s| !s.is_empty());
                if let Some(secret) = &supplied_secret {
                    if secret.len() > 10240 {
                        return Err(ConfigError::Invalid("credential exceeds byte limit".into()));
                    }
                    validation::text(secret, 64 * 1024, true).map_err(ConfigError::Invalid)?;
                }
                if supplied_secret.as_deref().or(material.api_key()).is_none() {
                    return Ok(P::Rejected(R::CredentialNotConfigured));
                }
                Ok(P::Ready(Box::new(PreparedOnboarding {
                    protocol,
                    store,
                    material,
                    original_revision,
                    requested_slug,
                    supplied_secret,
                })))
            })
        })
        .await
    }
}

fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{9}'..='\u{d}' | '\u{20}' | '\u{a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

impl PreparedOnboarding {
    pub fn network_configuration(&self) -> &crate::network::NetworkConfiguration {
        &self.material.network.configuration
    }
    pub fn protocol(&self) -> ModelListProtocol {
        self.protocol
    }
    pub fn connection(&self) -> &ConnectionCatalogEntry {
        &self.material.connection
    }
    pub fn endpoint(&self) -> &str {
        &self.material.endpoint
    }
    pub fn api_key(&self) -> &str {
        self.supplied_secret
            .as_deref()
            .or(self.material.api_key())
            .expect("prepared credential")
    }
    pub fn request_headers(&self) -> Option<&str> {
        self.material.request_headers()
    }
}
