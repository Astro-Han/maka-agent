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

impl Tokens {
    /// Decode a persisted credential, whose expiration is absolute milliseconds
    /// (unlike a provider response's relative expires_in). Extra provider fields
    /// are allowed; known credential fields remain bounded and typed.
    pub fn from_stored(raw: &str) -> Result<Self> {
        if raw.encode_utf16().count() > 64 * 1024 {
            return Err(ErrorKind::ResponseTooLarge.into());
        }
        let value: Value = serde_json::from_str(raw).map_err(|_| ErrorKind::InvalidResponse)?;
        if !value.is_object() {
            return Err(ErrorKind::InvalidResponse.into());
        }
        let optional = |key, limit| value.get(key).map(|v| text(v, limit)).transpose();
        let expires_at = value["expires_at"]
            .as_f64()
            .filter(|n| {
                n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n <= 9_007_199_254_740_991.0
            })
            .ok_or(ErrorKind::InvalidResponse)? as u64;
        Ok(Self {
            access_token: text(&value["access_token"], 32 * 1024)?,
            refresh_token: text(&value["refresh_token"], 32 * 1024)?,
            expires_at,
            id_token: optional("id_token", 32 * 1024)?,
            token_type: optional("token_type", 256)?,
            scope: optional("scope", 4096)?,
            base_url: optional("base_url", 8192)?,
            account_id: optional("account_id", 4096)?,
            account_uuid: optional("account_uuid", 4096)?,
        })
    }
}

impl Client {
    /// A refresh may spend a one-use grant. The Host must retain this future
    /// through settlement, persist the returned replacement before using it,
    /// and never automatically retry an uncertain outcome.
    pub async fn refresh(&self, provider: Provider, mut previous: Tokens) -> Result<Tokens> {
        if provider == Provider::GithubCopilot && previous.expires_at == 9_007_199_254_740_991 {
            return Ok(previous);
        }
        let mut request = self.http.post(token_endpoint(provider)).form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id(provider)),
            ("refresh_token", previous.refresh_token.as_str()),
        ]);
        request = match provider {
            Provider::OpenaiCodex => {
                request.header("user-agent", "maka-desktop/0.1.0 (oauth-subscription)")
            }
            Provider::GithubCopilot => request.header("accept", "application/json"),
            Provider::XaiOauth => request,
        };
        let response = self.request(request, None).await?;
        if !response.ok()
            || (provider == Provider::GithubCopilot && response.payload["error"].is_string())
        {
            return Err(response.rejected());
        }
        let value = &response.payload;
        if !value.is_object() {
            return Err(ErrorKind::InvalidResponse.into());
        }
        let access_token = text(&value["access_token"], 32 * 1024)?;
        if provider == Provider::GithubCopilot
            && !["gho_", "ghu_", "github_pat_"]
                .iter()
                .any(|prefix| access_token.starts_with(prefix))
        {
            return Err(ErrorKind::InvalidResponse.into());
        }
        let lifetime = value.get("expires_in").cloned().unwrap_or_else(|| {
            if provider == Provider::XaiOauth {
                Value::from(3600)
            } else {
                Value::Null
            }
        });
        let expires_at = expires(now()?, positive(&lifetime, 366 * 86400)?)?;
        let refresh_token = match value.get("refresh_token") {
            None => previous.refresh_token,
            Some(Value::String(value)) if value.is_empty() => previous.refresh_token,
            Some(value) => text(value, 32 * 1024)?,
        };
        let optional = |key, limit| value.get(key).map(|v| text(v, limit)).transpose();
        match provider {
            Provider::OpenaiCodex => Ok(Tokens {
                access_token,
                refresh_token,
                expires_at,
                id_token: optional("id_token", 32 * 1024)?.or(previous.id_token),
                account_id: previous.account_id,
                account_uuid: None,
                token_type: None,
                scope: None,
                base_url: None,
            }),
            Provider::XaiOauth => Ok(Tokens {
                access_token,
                refresh_token,
                expires_at,
                token_type: optional("token_type", 256)?.or(previous.token_type),
                scope: optional("scope", 4096)?.or(previous.scope),
                id_token: None,
                account_id: None,
                account_uuid: None,
                base_url: None,
            }),
            Provider::GithubCopilot => {
                previous.access_token = access_token;
                previous.refresh_token = refresh_token;
                previous.expires_at = expires_at;
                Ok(previous)
            }
        }
    }
}
