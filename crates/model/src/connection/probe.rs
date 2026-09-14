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

use super::{
    ConnectionClient, DiscoveryKind, DiscoveryRequest, Failure, customize_headers,
    generated_headers, network, read_bounded, status_failure,
};
use crate::{ProviderConfig, ProviderKind};
use reqwest::header::HeaderValue;
use serde_json::json;
use std::time::{Duration, Instant};

/// Contains measurements and classification, never the provider's error body.
#[derive(Debug)]
pub struct ProbeFailure {
    pub class: Failure,
    pub status_code: Option<u16>,
    pub elapsed: Duration,
}

impl ConnectionClient {
    /// Proves account inventory access and model availability, not inference.
    pub async fn test_inventory(
        &self,
        request: DiscoveryRequest<'_>,
        model: &str,
    ) -> Result<Duration, ProbeFailure> {
        let start = Instant::now();
        self.discover_inner(request)
            .await
            .and_then(|models| {
                if models.iter().any(|row| row.id == model) {
                    Ok(start.elapsed())
                } else {
                    Err((Failure::Unknown, None))
                }
            })
            .map_err(|(class, status_code)| ProbeFailure {
                class,
                status_code,
                elapsed: start.elapsed(),
            })
    }

    /// Success proves HTTP acceptance, not a valid completion payload.
    pub async fn test(&self, provider: &ProviderConfig) -> Result<Duration, ProbeFailure> {
        let start = Instant::now();
        self.test_inner(provider)
            .await
            .map(|()| start.elapsed())
            .map_err(|(class, status_code)| ProbeFailure {
                class,
                status_code,
                elapsed: start.elapsed(),
            })
    }

    async fn test_inner(&self, provider: &ProviderConfig) -> Result<(), (Failure, Option<u16>)> {
        let local = |class| (class, None);
        let kind = match &provider.kind {
            ProviderKind::Anthropic => DiscoveryKind::Anthropic,
            _ => DiscoveryKind::Openai,
        };
        let crate::ProviderAuth::ApiKey(api_key) = &provider.auth else {
            // This non-streaming probe does not establish subscription access.
            return Err(local(Failure::Unknown));
        };
        let mut headers = generated_headers(kind, api_key).map_err(local)?;
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        customize_headers(&mut headers, &provider.headers).map_err(local)?;
        let mut body = match &provider.kind {
            ProviderKind::OpenaiResponses => json!({
                "model":provider.model, "store":false, "max_output_tokens":16,
                "input":[{"role":"user","content":"Hi"}],
            }),
            _ => {
                json!({"model":provider.model,"max_tokens":16,"messages":[{"role":"user","content":"Hi"}]})
            }
        };
        if let Some(extra) = &provider.body_overlay {
            let body = body.as_object_mut().expect("probe object");
            if extra.keys().any(|key| body.contains_key(key)) {
                return Err(local(Failure::Network));
            }
            body.extend(
                extra
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        let url = match &provider.kind {
            ProviderKind::OpenaiResponses => {
                let mut url =
                    reqwest::Url::parse(&provider.base_url).map_err(|_| local(Failure::Unknown))?;
                let path = url.path().trim_end_matches('/');
                let path = if path.to_ascii_lowercase().ends_with("/responses") {
                    &path[..path.len() - 10]
                } else {
                    path
                };
                url.set_path(&format!("{path}/responses"));
                url.to_string()
            }
            ProviderKind::OpenaiChat | ProviderKind::OpenaiCompatible { .. } => format!(
                "{}/chat/completions",
                provider.base_url.trim_end_matches('/')
            ),
            ProviderKind::Anthropic => {
                format!("{}/messages", provider.base_url.trim_end_matches('/'))
            }
        };
        let response = self
            .client
            .post(url)
            .headers(headers)
            .timeout(Duration::from_secs(15))
            .body(serde_json::to_vec(&body).map_err(|_| local(Failure::Unknown))?)
            .send()
            .await
            .map_err(|error| local(network(error)))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        } // Drop cancels the unconsumed body.
        if status.as_u16() != 429 {
            read_bounded(response, 16 * 1024).await.map_err(local)?;
        }
        Err((status_failure(status.as_u16()), Some(status.as_u16())))
    }
}
