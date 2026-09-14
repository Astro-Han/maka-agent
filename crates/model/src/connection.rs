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

//! Native connection HTTP effects; catalog ownership and publication stay in the Host.
mod models;
mod probe;
pub(crate) mod subscription;
pub use probe::ProbeFailure;

use maka_runtime::configuration::{ConnectionEffectFailureClass as Failure, ModelInfo};
use reqwest::{
    Client,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use std::{collections::BTreeMap, time::Duration};

const BODY_LIMIT: usize = 4 * 1024 * 1024;

pub use maka_runtime::configuration::ModelListProtocol as DiscoveryKind;

/// Secret material has no Debug/Serialize implementation.
pub struct DiscoveryRequest<'a> {
    pub kind: DiscoveryKind,
    pub base_url: &'a str,
    pub credential: &'a str,
    pub headers: &'a BTreeMap<String, String>,
}

pub struct ConnectionClient {
    client: Client,
}

impl ConnectionClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::with_policy(&maka_network::Policy::default())
    }

    pub fn with_policy(policy: &maka_network::Policy) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: policy
                .client_builder()
                .timeout(Duration::from_secs(10))
                .build()?,
        })
    }

    pub async fn fetch(&self, request: DiscoveryRequest<'_>) -> Result<Vec<ModelInfo>, Failure> {
        let models = self.discover(request).await?;
        if models.is_empty() {
            return Err(Failure::InvalidResponse);
        }
        Ok(models)
    }

    pub async fn discover(&self, request: DiscoveryRequest<'_>) -> Result<Vec<ModelInfo>, Failure> {
        self.discover_inner(request)
            .await
            .map_err(|(class, _)| class)
    }

    async fn discover_inner(
        &self,
        request: DiscoveryRequest<'_>,
    ) -> Result<Vec<ModelInfo>, (Failure, Option<u16>)> {
        let local = |class| (class, None);
        let url = endpoint(request.kind, request.base_url);
        let mut headers = generated_headers(request.kind, request.credential).map_err(local)?;
        customize_headers(&mut headers, request.headers).map_err(local)?;
        let response = self
            .client
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(|error| local(network(error)))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err((status_failure(status), Some(status)));
        }
        let body = read_bounded(response, BODY_LIMIT).await.map_err(local)?;
        let models = models::decode(&body, request.kind).map_err(local)?;
        Ok(models)
    }
}

fn status_failure(status: u16) -> Failure {
    match status {
        401 | 403 => Failure::Auth,
        408 => Failure::Timeout,
        429 | 500..=599 => Failure::ProviderUnavailable,
        _ => Failure::Unknown,
    }
}

async fn read_bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, Failure> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Failure::InvalidResponse);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if chunk.len() > limit - body.len() {
            return Err(Failure::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn generated_headers(kind: DiscoveryKind, api_key: &str) -> Result<HeaderMap, Failure> {
    let mut headers = HeaderMap::new();
    match kind {
        DiscoveryKind::Openai => {
            headers.insert("content-type", HeaderValue::from_static("application/json"));
            if !api_key.is_empty() {
                headers.insert("authorization", header_value(&format!("Bearer {api_key}"))?);
            }
        }
        DiscoveryKind::Anthropic => {
            headers.insert("x-api-key", header_value(api_key)?);
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        }
        DiscoveryKind::Codex => return subscription::codex_headers(api_key),
        DiscoveryKind::Copilot => return subscription::copilot_headers(api_key),
    }
    Ok(headers)
}

fn customize_headers(
    headers: &mut HeaderMap,
    extra: &BTreeMap<String, String>,
) -> Result<(), Failure> {
    for (name, raw) in extra {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| Failure::Network)?;
        let value = header_value(raw)?;
        if headers
            .get(&name)
            .is_some_and(|generated| generated != value)
        {
            return Err(Failure::Network);
        }
        // Fetch normalizes surrounding HTTP whitespace when installing a header.
        headers.insert(name, header_value(raw.trim_matches([' ', '\t']))?);
    }
    Ok(())
}

fn header_value(value: &str) -> Result<HeaderValue, Failure> {
    // Fetch headers are ByteString, not UTF-8; the vault accepts Latin-1 values.
    let bytes = value
        .chars()
        .map(u8::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Failure::Network)?;
    let mut value = HeaderValue::from_bytes(&bytes).map_err(|_| Failure::Network)?;
    value.set_sensitive(true);
    Ok(value)
}

fn network(error: reqwest::Error) -> Failure {
    if error.is_timeout() {
        Failure::Timeout
    } else {
        Failure::Network
    }
}

fn endpoint(kind: DiscoveryKind, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match kind {
        DiscoveryKind::Openai | DiscoveryKind::Copilot => format!("{base}/models"),
        DiscoveryKind::Codex => format!("{base}/models?client_version=1.0.0"),
        DiscoveryKind::Anthropic => {
            let base = base
                .strip_suffix("/v1")
                .or_else(|| base.strip_suffix("/V1"))
                .unwrap_or(base);
            format!("{base}/v1/models")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_paths_do_not_apply_model_generation_url_rewrites() {
        assert_eq!(
            endpoint(DiscoveryKind::Openai, "https://example.org/v1/responses/"),
            "https://example.org/v1/responses/models"
        );
        for base in [
            "https://example.org/",
            "https://example.org/v1/",
            "https://example.org/V1",
        ] {
            assert_eq!(
                endpoint(DiscoveryKind::Anthropic, base),
                "https://example.org/v1/models"
            );
        }
    }
}
