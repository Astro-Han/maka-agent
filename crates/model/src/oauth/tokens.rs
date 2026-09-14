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
use serde::Serialize;

const TOKEN_LIMIT: usize = 32 * 1024;
/// Secret-bearing material: deliberately no Debug implementation.
#[derive(Serialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_uuid: Option<String>,
}
impl Tokens {
    pub(super) fn decode(provider: Provider, payload: &Value) -> Result<Self> {
        if !payload.is_object() {
            return Err(ErrorKind::InvalidResponse.into());
        }
        let access_token = text(&payload["access_token"], TOKEN_LIMIT)?;
        let optional = |field, limit| payload.get(field).map(|v| text(v, limit)).transpose();
        if provider == Provider::GithubCopilot {
            if !["gho_", "ghu_", "github_pat_"]
                .iter()
                .any(|prefix| access_token.starts_with(prefix))
            {
                return Err(ErrorKind::InvalidResponse.into());
            }
            let (refresh_token, expires_at) = if let Some(lifetime) = payload.get("expires_in") {
                (
                    text(&payload["refresh_token"], TOKEN_LIMIT)?,
                    expires(now()?, positive(lifetime, 366 * 86400)?)?,
                )
            } else {
                (access_token.clone(), 9_007_199_254_740_991)
            };
            return Ok(Self {
                access_token,
                refresh_token,
                expires_at,
                id_token: None,
                token_type: Some("Bearer".into()),
                scope: None,
                base_url: Some("https://api.githubcopilot.com".into()),
                account_id: None,
                account_uuid: None,
            });
        }
        Ok(Self {
            access_token,
            refresh_token: text(&payload["refresh_token"], TOKEN_LIMIT)?,
            expires_at: expires(now()?, positive(&payload["expires_in"], 366 * 86400)?)?,
            id_token: optional("id_token", TOKEN_LIMIT)?,
            token_type: optional("token_type", 256)?,
            scope: optional("scope", 4096)?,
            base_url: None,
            account_id: None,
            account_uuid: None,
        })
    }
}
