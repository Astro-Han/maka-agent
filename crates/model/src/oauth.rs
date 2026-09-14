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

//! Native provider authorization. The Host owns presentation, cancellation and
//! persistence; this transport never starts V8 or writes credentials.
mod copilot;
mod device;
mod http;
mod refresh;
mod tokens;
pub use device::DeviceAuthorization;
use maka_runtime::oauth::Provider;
use reqwest::{Client as HttpClient, ClientBuilder};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
pub use tokens::Tokens;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidGrant,
    InvalidToken,
    ProviderRejected,
    InvalidResponse,
    ResponseTooLarge,
    Aborted,
    OutcomeUnknown,
    Expired,
    EntitlementDenied,
    EntitlementUnavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("OAuth authorization failed: {kind:?} (status {status:?})")]
pub struct Error {
    pub kind: ErrorKind,
    pub status: Option<u16>,
}
type Result<T> = std::result::Result<T, Error>;
impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self { kind, status: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollBoundary {
    Admitted,
    Retry,
    Exchanging,
}

#[derive(Clone)]
pub struct Client {
    http: HttpClient,
}
impl Client {
    pub fn new(policy: &maka_network::Policy) -> std::result::Result<Self, reqwest::Error> {
        Self::with_http_builder(policy.client_builder())
    }
    /// Trusted transport injection for private CAs and tests. Callers must install
    /// the root's routing policy; grant requests always disable redirect/retry.
    pub fn with_http_builder(builder: ClientBuilder) -> std::result::Result<Self, reqwest::Error> {
        Ok(Self {
            http: builder
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(15))
                .build()?,
        })
    }
}

fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|v| u64::try_from(v.as_millis()).ok())
        .filter(|v| *v <= 9_007_199_254_740_991)
        .ok_or_else(|| ErrorKind::InvalidResponse.into())
}
fn text(value: &Value, limit: usize) -> Result<String> {
    value
        .as_str()
        .filter(|v| !v.is_empty() && v.encode_utf16().count() <= limit)
        .map(str::to_owned)
        .ok_or_else(|| ErrorKind::InvalidResponse.into())
}
fn positive(value: &Value, max: u64) -> Result<u64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && v.fract() == 0.0 && *v > 0.0 && *v <= max as f64)
        .map(|v| v as u64)
        .ok_or_else(|| ErrorKind::InvalidResponse.into())
}
fn expires(now: u64, seconds: u64) -> Result<u64> {
    now.checked_add(
        seconds
            .checked_mul(1000)
            .ok_or(ErrorKind::InvalidResponse)?,
    )
    .filter(|v| *v <= 9_007_199_254_740_991)
    .ok_or_else(|| ErrorKind::InvalidResponse.into())
}
fn client_id(provider: Provider) -> &'static str {
    match provider {
        Provider::OpenaiCodex => "app_EMoamEEZ73f0CkXaXp7hrann",
        Provider::GithubCopilot => "Iv1.b507a08c87ecfe98",
        Provider::XaiOauth => "b1a00492-073a-47ea-816f-4c329264a828",
    }
}
fn token_endpoint(provider: Provider) -> &'static str {
    match provider {
        Provider::OpenaiCodex => "https://auth.openai.com/oauth/token",
        Provider::GithubCopilot => "https://github.com/login/oauth/access_token",
        Provider::XaiOauth => "https://auth.x.ai/oauth2/token",
    }
}
