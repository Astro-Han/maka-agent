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

use super::{Host, configuration};
use maka_config::model_fetch::ModelFetchPreparation;
use maka_model::connection::{ConnectionClient, DiscoveryRequest};
use maka_protocol::{OperationError, OperationErrorCode};
use maka_runtime::configuration::ConnectionModelFetchResult;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

mod test;

/// Serialize connection effects, not ordinary catalog edits or network-wide traffic.
#[derive(Default)]
pub(super) struct ConnectionEffects {
    lanes: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

impl ConnectionEffects {
    pub(super) async fn lane(&self, id: &str) -> OwnedMutexGuard<()> {
        let lane = {
            let mut lanes = self.lanes.lock().expect("connection effect lanes poisoned");
            lanes.retain(|_, lane| lane.strong_count() > 0);
            match lanes.get(id).and_then(Weak::upgrade) {
                Some(lane) => lane,
                None => {
                    let lane = Arc::new(AsyncMutex::new(()));
                    lanes.insert(id.to_owned(), Arc::downgrade(&lane));
                    lane
                }
            }
        };
        lane.lock_owned().await
    }

    pub async fn fetch(
        &self,
        host: &Host,
        id: &str,
    ) -> Result<ConnectionModelFetchResult, OperationError> {
        let _lane = self.lane(id).await;
        // Serialize snapshot/binding with committed connection deletion only;
        // no network request or refresh settlement runs under admission.
        let admission = host.executions.lock_admission().await;
        let mut prepared = match host
            .configuration
            .prepare_model_fetch(id)
            .await
            .map_err(configuration::failure)?
        {
            ModelFetchPreparation::Ready(prepared) => prepared,
            ModelFetchPreparation::Rejected(reason) => {
                return Ok(ConnectionModelFetchResult::Rejected { reason });
            }
            ModelFetchPreparation::Unsupported => {
                return Err(OperationError {
                    code: OperationErrorCode::OperationUnavailable,
                    message: "Provider model discovery is not installed".into(),
                });
            }
        };
        let authentication = prepared
            .oauth_credential()
            .map(|snapshot| bind_oauth(host, snapshot))
            .transpose()?;
        drop(admission);
        let access_token = if let Some((credential, client)) = authentication {
            let resolved = credential.resolve(client).await.map_err(oauth_failure)?;
            prepared
                .accept_oauth(resolved.credential)
                .map_err(configuration::failure)?;
            Some(resolved.access_token)
        } else {
            None
        };
        let kind = prepared.protocol();
        let headers = prepared
            .request_headers()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| OperationError {
                code: OperationErrorCode::InternalFailure,
                message: "Stored request headers are invalid".into(),
            })?
            .unwrap_or_default();
        let result = client(prepared.network_configuration())?
            .fetch(DiscoveryRequest {
                kind,
                base_url: prepared.endpoint(),
                credential: access_token
                    .as_deref()
                    .unwrap_or_else(|| prepared.api_key()),
                headers: &headers,
            })
            .await;
        match result {
            Ok(models) => prepared
                .complete(
                    models,
                    configuration::now().map_err(configuration::failure)?,
                )
                .await
                .map_err(configuration::failure),
            Err(error_class) => Ok(ConnectionModelFetchResult::Failed { error_class }),
        }
    }
}

/// Called under the existing admission gate; the returned refresh runs outside it.
fn bind_oauth(
    host: &Host,
    snapshot: maka_config::oauth::OAuthCredential,
) -> Result<(crate::oauth::Credential, maka_model::oauth::Client), OperationError> {
    let provider = serde_json::from_value(serde_json::Value::String(
        snapshot.target().provider_type.clone(),
    ))
    .map_err(|_| OperationError {
        code: OperationErrorCode::OperationUnavailable,
        message: "Provider OAuth refresh is not installed".into(),
    })?;
    let settings = snapshot.network_configuration();
    let policy = maka_network::Policy::from_settings(&settings.proxy, settings.password.as_deref())
        .map_err(oauth_failure)?;
    let client = maka_model::oauth::Client::new(&policy).map_err(oauth_failure)?;
    let credential = host
        .executions
        .oauth
        .bind(snapshot, provider)
        .map_err(oauth_failure)?;
    Ok((credential, client))
}

fn oauth_failure(error: impl std::fmt::Display) -> OperationError {
    OperationError {
        code: OperationErrorCode::OperationUnavailable,
        message: error.to_string(),
    }
}

pub(super) fn client(
    configuration: &maka_config::network::NetworkConfiguration,
) -> Result<ConnectionClient, OperationError> {
    let policy = maka_network::Policy::from_settings(
        &configuration.proxy,
        configuration.password.as_deref(),
    )
    .map_err(|error| OperationError {
        code: OperationErrorCode::OperationUnavailable,
        message: error.to_string(),
    })?;
    ConnectionClient::with_policy(&policy).map_err(|_| OperationError {
        code: OperationErrorCode::InternalFailure,
        message: "Cannot initialize network client".into(),
    })
}
