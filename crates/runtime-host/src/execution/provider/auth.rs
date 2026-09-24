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
use maka_model::{AuthResolver, ModelError, ProviderAuth};
use maka_plugins::provider::{Binding as ProviderBinding, Connection, Context};
use std::{future::Future, pin::Pin};

pub(super) struct Binding {
    snapshot: Option<maka_config::oauth::ProviderCredential>,
    credential: std::sync::OnceLock<Option<crate::oauth::Credential>>,
    provider: ProviderBinding,
    connection: Connection,
    transport: Arc<dyn maka_plugins::model::Transport>,
    session_id: String,
}

impl Binding {
    pub fn auth(self: &Arc<Self>) -> Result<ProviderAuth, OperationError> {
        let generation = self
            .snapshot
            .as_ref()
            .map(|snapshot| (&snapshot.basis().locator, &snapshot.basis().credential_id));
        Ok(ProviderAuth::Bound {
            identity: serde_json::to_string(&(
                "maka.provider.execution.v1",
                self.provider.identity(),
                &self.connection.id,
                generation,
            ))
            .map_err(|_| unavailable("Invalid provider credential identity"))?,
            resolver: self.clone(),
        })
    }

    /// Caller holds Host admission. Pure observations cannot start refresh work.
    pub fn admit(&self, authority: &crate::oauth::Authority) -> Result<(), OperationError> {
        let credential = self
            .snapshot
            .clone()
            .map(|snapshot| authority.bind(snapshot))
            .transpose()
            .map_err(|error| unavailable(error.to_string()))?;
        self.credential
            .set(credential)
            .map_err(|_| unavailable("Provider binding was already admitted"))
    }
}

impl AuthResolver for Binding {
    fn resolve(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderAuth, ModelError>> + Send + '_>> {
        Box::pin(async move {
            let admitted = self.credential.get().ok_or_else(|| {
                ModelError::Adapter("Provider observation is not admitted".into())
            })?;
            let credential = if let Some(admitted) = admitted {
                Some(
                    admitted
                        .resolve(
                            self.provider.clone(),
                            Context {
                                transport: self.transport.clone(),
                                cancellation: tokio_util::sync::CancellationToken::new(),
                                interaction: None,
                            },
                        )
                        .await?
                        .credential()
                        .clone(),
                )
            } else {
                None
            };
            let credentials = self
                .provider
                .authorize(self.connection.clone(), credential, self.session_id.clone())
                .await
                .map_err(|error| ModelError::Adapter(error.to_string()))?;
            Ok(match credentials {
                maka_plugins::model::Credentials::ApiKey(key) => ProviderAuth::ApiKey(key),
                maka_plugins::model::Credentials::RequestHeaders(headers) => {
                    ProviderAuth::RequestHeaders(headers)
                }
            })
        })
    }
}

pub(super) fn observe(
    models: &maka_model::ModelExecutor,
    provider: ProviderBinding,
    material: &maka_config::ConnectionObservation,
    session_id: &str,
) -> Result<Arc<Binding>, OperationError> {
    if provider.identity() != &material.connection.provider {
        return Err(unavailable(
            "Provider does not match the connection recipient",
        ));
    }
    let settings = &material.network;
    let policy =
        maka_network::Policy::from_host_settings(&settings.proxy, settings.password.as_deref())
            .map_err(|error| unavailable(error.to_string()))?;
    let transport = models
        .transport(&policy)
        .map_err(|error| unavailable(error.to_string()))?;
    Ok(Arc::new(Binding {
        snapshot: material.credential.clone(),
        credential: std::sync::OnceLock::new(),
        provider,
        connection: Connection {
            id: material.connection.connection_id.clone(),
            revision: material.connection.revision,
            configuration: material.connection.configuration.clone(),
        },
        transport,
        session_id: session_id.into(),
    }))
}
