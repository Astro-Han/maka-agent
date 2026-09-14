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

mod poll;
use super::*;
use serde_json::json;

/// An unpublished device grant. Host presentation may read only the user code
/// and verification URL; finishing consumes the handle.
pub struct DeviceAuthorization {
    client: Client,
    provider: Provider,
    code: String,
    user_code: String,
    verification_url: String,
    expires_at: u64,
    interval: u64,
}
impl DeviceAuthorization {
    pub fn user_code(&self) -> &str {
        &self.user_code
    }
    pub fn verification_url(&self) -> &str {
        &self.verification_url
    }
}
impl Client {
    pub async fn start(
        &self,
        provider: Provider,
        cancel: &CancellationToken,
    ) -> Result<DeviceAuthorization> {
        let request = match provider {
            Provider::OpenaiCodex => self
                .http
                .post("https://auth.openai.com/api/accounts/deviceauth/usercode")
                .header("content-type", "application/json")
                .body(json!({"client_id":client_id(provider)}).to_string()),
            Provider::GithubCopilot => self
                .http
                .post("https://github.com/login/device/code")
                .header("accept", "application/json")
                .form(&[("client_id", client_id(provider)), ("scope", "read:user")]),
            Provider::XaiOauth => self
                .http
                .post("https://auth.x.ai/oauth2/device/code")
                .form(&[
                    ("client_id", client_id(provider)),
                    (
                        "scope",
                        "openid profile email offline_access grok-cli:access api:access",
                    ),
                    ("referrer", "maka"),
                ]),
        };
        let response = self.request(request, Some(cancel)).await?;
        if !response.ok() || (provider == Provider::GithubCopilot && response.code().is_some()) {
            return Err(response.failure(ErrorKind::ProviderRejected));
        }
        let payload = &response.payload;
        if !payload.is_object() {
            return Err(ErrorKind::InvalidResponse.into());
        }
        let now = now()?;
        let (code, user_code, verification_url, expires_at, interval) =
            if provider == Provider::OpenaiCodex {
                let interval = if let Some(s) = payload["interval"].as_str() {
                    positive(
                        &serde_json::to_value(
                            s.trim()
                                .parse::<f64>()
                                .map_err(|_| ErrorKind::InvalidResponse)?,
                        )
                        .map_err(|_| ErrorKind::InvalidResponse)?,
                        9_007_199_254_740,
                    )?
                } else {
                    positive(&payload["interval"], 9_007_199_254_740)?
                };
                let expires_at = payload["expires_at"]
                    .as_str()
                    .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                    .and_then(|v| u64::try_from(v.timestamp_millis()).ok())
                    .filter(|v| *v > now)
                    .unwrap_or(expires(now, 15 * 60)?);
                (
                    text(&payload["device_auth_id"], 1024)?,
                    text(
                        payload
                            .get("user_code")
                            .filter(|v| !v.is_null())
                            .unwrap_or(&payload["usercode"]),
                        1024,
                    )?,
                    "https://auth.openai.com/codex/device".into(),
                    expires_at,
                    interval * 1000,
                )
            } else {
                let uri = if provider == Provider::XaiOauth {
                    payload
                        .get("verification_uri_complete")
                        .filter(|v| !v.is_null())
                        .unwrap_or(&payload["verification_uri"])
                } else {
                    &payload["verification_uri"]
                };
                let verification_url = text(uri, 8192)?;
                let url = reqwest::Url::parse(&verification_url)
                    .map_err(|_| ErrorKind::InvalidResponse)?;
                let domain = if provider == Provider::XaiOauth {
                    "x.ai"
                } else {
                    "github.com"
                };
                if url.scheme() != "https"
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || !url
                        .host_str()
                        .is_some_and(|host| host == domain || host.ends_with(&format!(".{domain}")))
                {
                    return Err(ErrorKind::InvalidResponse.into());
                }
                (
                    text(&payload["device_code"], 32 * 1024)?,
                    text(&payload["user_code"], 1024)?,
                    verification_url,
                    expires(now, positive(&payload["expires_in"], 86400)?)?,
                    payload
                        .get("interval")
                        .map(|v| positive(v, 300))
                        .transpose()?
                        .unwrap_or(5)
                        * 1000,
                )
            };
        Ok(DeviceAuthorization {
            client: self.clone(),
            provider,
            code,
            user_code,
            verification_url,
            expires_at,
            interval,
        })
    }
}
