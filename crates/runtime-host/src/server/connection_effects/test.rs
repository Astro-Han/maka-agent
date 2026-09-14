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

use super::{ConnectionEffects, Host, configuration};
use crate::provider_route;
use maka_config::{
    connection_test::{ConnectionTestPreparation, PreparedConnectionTest},
    model_catalog::{self, ProviderFacts},
};
use maka_model::{
    ProviderConfig,
    connection::{DiscoveryKind, DiscoveryRequest},
};
use maka_protocol::{OperationError, OperationErrorCode};
use maka_runtime::configuration::{
    ConnectionCatalogEntry, ConnectionEffectFailureClass as Failure, ConnectionTestProjection,
    ConnectionTestRunInput, ConnectionTestRunResult, ModelDiscoverySource,
};

impl ConnectionEffects {
    pub async fn test(
        &self,
        host: &Host,
        input: ConnectionTestRunInput,
    ) -> Result<ConnectionTestRunResult, OperationError> {
        let _lane = self.lane(&input.connection_id).await;
        let admission = host.executions.lock_admission().await;
        let mut prepared = match host
            .configuration
            .prepare_connection_test(input)
            .await
            .map_err(configuration::failure)?
        {
            ConnectionTestPreparation::Ready(prepared) => prepared,
            ConnectionTestPreparation::Rejected(reason) => {
                return Ok(ConnectionTestRunResult::Rejected { reason });
            }
            ConnectionTestPreparation::Unsupported => {
                return Err(OperationError {
                    code: OperationErrorCode::OperationUnavailable,
                    message: "Provider connection testing is not installed".into(),
                });
            }
        };
        let start = std::time::Instant::now();
        let authentication = prepared
            .oauth_credential()
            .map(|snapshot| super::bind_oauth(host, snapshot))
            .transpose()?;
        drop(admission);
        let access_token = if let Some((credential, client)) = authentication {
            let resolved = credential
                .resolve(client)
                .await
                .map_err(super::oauth_failure)?;
            prepared
                .accept_oauth(resolved.credential)
                .map_err(configuration::failure)?;
            Some(resolved.access_token)
        } else {
            None
        };
        let facts = model_catalog::provider_facts(&prepared.connection().provider_type)
            .map_err(configuration::failure)?;
        let model = select_model(prepared.connection(), facts, prepared.model_id());
        let result = match model {
            None => Err((Failure::Unknown, None, None)),
            Some(model) => {
                let credential = access_token
                    .as_deref()
                    .unwrap_or_else(|| prepared.api_key());
                let observed = match (access_token.is_some(), facts.native_model_list()) {
                    // The source contract checks resolved OAuth readiness. Tiny
                    // synthetic Codex POSTs are not stable inference probes.
                    (true, Some(DiscoveryKind::Codex)) => Ok(start.elapsed()),
                    (true, Some(DiscoveryKind::Copilot)) => {
                        let headers = headers(&prepared)?;
                        super::client(prepared.network_configuration())?
                            .test_inventory(
                                DiscoveryRequest {
                                    kind: DiscoveryKind::Copilot,
                                    base_url: prepared.endpoint(),
                                    credential,
                                    headers: &headers,
                                },
                                model,
                            )
                            .await
                    }
                    _ => {
                        // Unsupported local routing is not a failed provider observation.
                        let provider = provider(&prepared, facts, model, credential)?;
                        super::client(prepared.network_configuration())?
                            .test(&provider)
                            .await
                    }
                };
                observed
                    .map(|elapsed| (model.to_owned(), millis(elapsed)))
                    .map_err(|error| {
                        (
                            error.class,
                            error.status_code.map(u64::from),
                            Some(millis(error.elapsed)),
                        )
                    })
            }
        };
        let checked_at = time::OffsetDateTime::now_utc()
            .format(time::macros::format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
            ))
            .map_err(|_| OperationError {
                code: OperationErrorCode::InternalFailure,
                message: "Cannot format connection test timestamp".into(),
            })?;
        let test = match result {
            Ok((model_id, latency_ms)) => ConnectionTestProjection::Verified {
                checked_at,
                model_id,
                latency_ms,
            },
            Err((error_class, status_code, latency_ms)) => ConnectionTestProjection::Failed {
                checked_at,
                model_id: None,
                latency_ms,
                status_code,
                error_class,
            },
        };
        prepared
            .complete(test)
            .await
            .map_err(configuration::failure)
    }
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).expect("bounded connection probe duration")
}

fn provider(
    prepared: &PreparedConnectionTest,
    facts: &ProviderFacts,
    model: &str,
    credential: &str,
) -> Result<ProviderConfig, OperationError> {
    let route = provider_route::resolve(prepared.connection(), facts, model)?;
    route.check_probe()?;
    Ok(ProviderConfig {
        kind: route.kind,
        model: model.to_owned(),
        base_url: route.base_url,
        auth: maka_model::ProviderAuth::ApiKey(credential.to_owned()),
        headers: headers(prepared)?,
        network: maka_network::Policy::from_settings(
            &prepared.network_configuration().proxy,
            prepared.network_configuration().password.as_deref(),
        )
        .map_err(|error| OperationError {
            code: OperationErrorCode::OperationUnavailable,
            message: error.to_string(),
        })?,
        body_overlay: prepared
            .connection()
            .request_body_overlay
            .as_ref()
            .map(|value| {
                value
                    .as_object()
                    .expect("validated request body overlay")
                    .clone()
            }),
    })
}

fn headers(
    prepared: &PreparedConnectionTest,
) -> Result<std::collections::BTreeMap<String, String>, OperationError> {
    prepared
        .request_headers()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| OperationError {
            code: OperationErrorCode::InternalFailure,
            message: "Stored request headers are invalid".into(),
        })
        .map(Option::unwrap_or_default)
}

/// A fetched inventory orders configured candidates; it never removes them.
fn select_model<'a>(
    row: &'a ConnectionCatalogEntry,
    facts: &'a ProviderFacts,
    explicit: Option<&'a str>,
) -> Option<&'a str> {
    let nonempty = |id: &'a str| {
        let id = id.trim();
        (!id.is_empty()).then_some(id)
    };
    if let Some(id) = explicit.and_then(nonempty) {
        return Some(id);
    }
    if row.model_source == Some(ModelDiscoverySource::Fetched)
        && facts.supports_model_discovery
        && let Some(id) = row
            .enabled_model_ids
            .iter()
            .filter_map(|id| nonempty(id))
            .find(|id| row.models.iter().any(|model| model.id.trim() == *id))
    {
        return Some(id);
    }
    row.enabled_model_ids
        .iter()
        .map(String::as_str)
        .chain(facts.fallback_models.iter().map(String::as_str))
        .chain(row.models.iter().map(|model| model.id.as_str()))
        .find_map(nonempty)
}
