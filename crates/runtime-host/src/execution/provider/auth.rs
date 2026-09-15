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
use maka_runtime::oauth::Provider;
use std::{future::Future, pin::Pin};

pub(super) struct Binding {
    snapshot: maka_config::oauth::OAuthCredential,
    credential: std::sync::OnceLock<crate::oauth::Credential>,
    client: maka_model::oauth::Client,
    provider: Provider,
    session_id: String,
}

impl Binding {
    pub fn auth(self: &Arc<Self>) -> Result<ProviderAuth, OperationError> {
        let basis = self.snapshot.basis();
        Ok(ProviderAuth::Bound {
            // Refresh advances the secret revision, not the login generation.
            // Enrollment always mints a new ID; the resolver still validates its
            // real current basis and cannot revive a revoked generation.
            identity: serde_json::to_string(&(
                "maka.oauth.execution.v1",
                self.provider,
                &basis.locator,
                &basis.credential_id,
            ))
            .map_err(|_| unavailable("Invalid OAuth credential identity"))?,
            resolver: self.clone(),
        })
    }

    /// Caller holds Host admission, before exposing a new execution. A query
    /// never admits this observation or changes the root's current generation.
    pub fn admit(&self, authority: &crate::oauth::Authority) -> Result<(), OperationError> {
        let credential = authority
            .bind(self.snapshot.clone(), self.provider)
            .map_err(|error| unavailable(error.to_string()))?;
        self.credential
            .set(credential)
            .map_err(|_| unavailable("OAuth binding was already admitted"))
    }
}

impl AuthResolver for Binding {
    fn resolve(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderAuth, ModelError>> + Send + '_>> {
        Box::pin(async move {
            let credential = self.credential.get().ok_or_else(|| {
                ModelError::Adapter("OAuth observation is not admitted for execution".into())
            })?;
            let access_token = credential.access_token(self.client.clone()).await?;
            Ok(match self.provider {
                Provider::OpenaiCodex => ProviderAuth::Codex {
                    access_token,
                    session_id: self.session_id.clone(),
                },
                Provider::XaiOauth => ProviderAuth::ApiKey(access_token),
                Provider::GithubCopilot => {
                    return Err(ModelError::Adapter(
                        "Copilot model profile is not installed".into(),
                    ));
                }
            })
        })
    }
}

pub(super) async fn observe(
    config: &Arc<ConfigurationStore>,
    target: ConnectionCredentialTarget,
    session_id: &str,
) -> Result<Arc<Binding>, OperationError> {
    let provider = match target.provider_type.as_str() {
        "openai-codex" => Provider::OpenaiCodex,
        "xai-oauth" => Provider::XaiOauth,
        _ => return Err(unavailable("Provider OAuth model profile is not installed")),
    };
    let snapshot = config
        .oauth_credential(target)
        .await
        .map_err(crate::server::configuration::failure)?
        .ok_or_else(|| unavailable("OAuth credential connection is no longer available"))?;
    let settings = snapshot.network_configuration();
    let policy = maka_network::Policy::from_settings(&settings.proxy, settings.password.as_deref())
        .map_err(|error| unavailable(error.to_string()))?;
    let client =
        maka_model::oauth::Client::new(&policy).map_err(|error| unavailable(error.to_string()))?;
    Ok(Arc::new(Binding {
        snapshot,
        credential: std::sync::OnceLock::new(),
        client,
        provider,
        session_id: session_id.into(),
    }))
}
