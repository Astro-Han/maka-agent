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

use super::{Failure, HeaderMap, HeaderValue, header_value};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;

pub(crate) fn copilot_headers(token: &str) -> Result<HeaderMap, Failure> {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", header_value(&format!("Bearer {token}"))?);
    for (name, value) in [
        ("user-agent", "GitHubCopilotChat/0.35.0"),
        ("editor-version", "vscode/1.107.0"),
        ("editor-plugin-version", "copilot-chat/0.35.0"),
        ("copilot-integration-id", "vscode-chat"),
        ("openai-intent", "conversation-edits"),
        ("x-github-api-version", "2026-06-01"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    Ok(headers)
}

pub(super) fn codex_headers(token: &str) -> Result<HeaderMap, Failure> {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", header_value(&format!("Bearer {token}"))?);
    for (name, value) in [
        ("content-type", "application/json"),
        ("openai-beta", "responses=experimental"),
        ("originator", "codex_cli_rs"),
        ("user-agent", "codex_cli_rs/0.0.0 (Maka)"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    if let Some(account) = codex_account(token) {
        headers.insert("chatgpt-account-id", header_value(&account)?);
    }
    Ok(headers)
}

// This is request routing, not JWT verification. A subject is NEVER an account ID.
fn codex_account(token: &str) -> Option<String> {
    let mut parts = token.split('.');
    parts.next()?;
    let payload = parts.next()?;
    parts.next()?;
    if parts.next().is_some() || payload.len() > 64 * 1024 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let direct = claims
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    let nested = claims
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    direct.or(nested).map(str::to_owned).or_else(|| {
        claims
            .get("organizations")?
            .as_array()?
            .iter()
            .find_map(|org| {
                org.get("id")?
                    .as_str()
                    .map(|id| id.trim_matches(super::models::js_whitespace))
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
            })
    })
}
