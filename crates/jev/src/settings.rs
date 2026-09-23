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

use maka_plugins::{credentials, storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub url: String,
    pub model: String,
    pub timeout_ms: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "https://api.typesafe.ai/v1/systemone".into(),
            model: "jev-latest".into(),
            timeout_ms: 8000,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), storage::StoreError> {
        let url = url::Url::parse(&self.url).map_err(|_| invalid())?;
        if self.url.len() > 8192
            || !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || self.model.trim().is_empty()
            || self.model.len() > 256
            || self.model.chars().any(char::is_control)
            || !(100..=60000).contains(&self.timeout_ms)
        {
            return Err(invalid());
        }
        Ok(())
    }
    // Changing the destination must never forward a credential saved for another endpoint.
    pub(crate) fn credential_key(&self) -> String {
        format!("endpoint-{:x}", Sha256::digest(self.url.as_bytes()))
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: Option<u64>,
    pub settings: Settings,
    pub credential_revision: Option<u64>,
    pub configured: bool,
    pub header_names: Vec<String>,
}
#[derive(Clone)]
pub(crate) struct Repository {
    pub store: Arc<dyn storage::Store>,
    pub credentials: Arc<dyn credentials::Credentials>,
}
impl Repository {
    pub async fn settings(&self) -> Result<(Option<u64>, Settings), storage::StoreError> {
        let record = self.store.read("settings".into()).await?;
        let revision = record.as_ref().map(|r| r.revision);
        let settings = record
            .and_then(|r| r.data.value().cloned())
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| invalid())?
            .unwrap_or_default();
        Settings::validate(&settings)?;
        Ok((revision, settings))
    }
    pub async fn snapshot(&self) -> Result<Snapshot, storage::StoreError> {
        let (revision, settings) = self.settings().await?;
        let credential = self.credentials.read(settings.credential_key()).await?;
        Ok(Snapshot {
            revision,
            settings,
            credential_revision: credential.as_ref().map(|r| r.revision),
            header_names: credential
                .as_ref()
                .and_then(|r| r.secret.as_ref())
                .map(|s| serde_json::from_str::<Secrets>(s))
                .transpose()
                .map_err(|_| invalid())?
                .map(|s| s.headers.into_keys().collect())
                .unwrap_or_default(),
            configured: credential.and_then(|r| r.secret).is_some(),
        })
    }
    pub async fn save(
        &self,
        expected: Option<u64>,
        settings: Settings,
    ) -> Result<(), storage::StoreError> {
        settings.validate()?;
        self.store
            .batch(vec![storage::Mutation {
                key: "settings".into(),
                expected_revision: expected,
                data: storage::Data::Present(
                    serde_json::to_value(settings).map_err(|_| invalid())?,
                ),
            }])
            .await?;
        Ok(())
    }
}
fn invalid() -> storage::StoreError {
    storage::StoreError::Unavailable("Invalid Jev settings: use an HTTP(S) URL without user credentials or fragment, a model, and a timeout from 100 to 60000 ms".into())
}

/// Endpoint-bound request secrets. Never returned by a settings read.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Secrets {
    pub api_key: Option<String>,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}
impl Secrets {
    pub fn validate(&self) -> Result<(), storage::StoreError> {
        if self.headers.len() > 32
            || self
                .api_key
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 4096 || s.chars().any(char::is_control))
        {
            return Err(invalid_headers());
        }
        let mut names = std::collections::BTreeSet::new();
        for (name, value) in &self.headers {
            let lower = name.to_ascii_lowercase();
            if name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                || value.len() > 4096
                || value.chars().any(char::is_control)
                || !names.insert(lower.clone())
                || matches!(
                    lower.as_str(),
                    "host"
                        | "content-length"
                        | "transfer-encoding"
                        | "connection"
                        | "content-type"
                        | "proxy-authorization"
                )
                || (lower == "authorization" && self.api_key.is_some())
            {
                return Err(invalid_headers());
            }
        }
        Ok(())
    }
    pub fn request_headers(&self) -> Result<Vec<(String, String)>, storage::StoreError> {
        self.validate()?;
        let mut headers: Vec<_> = self
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        headers.push(("Content-Type".into(), "application/json".into()));
        if let Some(key) = &self.api_key {
            headers.push(("Authorization".into(), format!("Bearer {key}")));
        }
        Ok(headers)
    }
}
fn invalid_headers() -> storage::StoreError {
    storage::StoreError::Unavailable("Invalid Jev authentication or request headers".into())
}
