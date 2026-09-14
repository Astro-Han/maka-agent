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
impl Client {
    /// A GitHub grant proves the account, not access to a usable Copilot model.
    /// A transport/server/malformed-response failure cannot prove ineligibility.
    pub(super) async fn verify_copilot(&self, tokens: &Tokens) -> Result<()> {
        let mut response = self
            .http
            .get("https://api.githubcopilot.com/models")
            .headers(
                crate::connection::subscription::copilot_headers(&tokens.access_token)
                    .map_err(|_| Error::from(ErrorKind::EntitlementUnavailable))?,
            )
            .send()
            .await
            .map_err(|_| Error::from(ErrorKind::EntitlementUnavailable))?;
        let status = response.status().as_u16();
        let unavailable = || Error {
            kind: ErrorKind::EntitlementUnavailable,
            status: Some(status),
        };
        let denied = || Error {
            kind: ErrorKind::EntitlementDenied,
            status: Some(status),
        };
        if matches!(status, 401 | 403) {
            return Err(denied());
        }
        if !response.status().is_success() {
            return Err(unavailable());
        }
        // Model inventories have their own budget, independent of small token JSON.
        const LIMIT: usize = 4 * 1024 * 1024;
        if response.content_length().is_some_and(|n| n > LIMIT as u64) {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if chunk.len() > LIMIT - bytes.len() {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let payload: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        let models = payload
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(unavailable)?;
        let mut usable = false;
        for model in models {
            if !model.is_object() {
                return Err(unavailable());
            }
            if !model["id"].as_str().is_some_and(|s| !s.is_empty())
                || model["model_picker_enabled"] != true
                || model
                    .get("policy")
                    .is_some_and(|p| !p.is_object() || p["state"] != "enabled")
                || model["capabilities"]["supports"]["tool_calls"] != true
            {
                continue;
            }
            for value in [
                model.get("supported_endpoints"),
                model.pointer("/capabilities/supports/reasoning_effort"),
                model.pointer("/capabilities/limits/vision/supported_media_types"),
            ]
            .into_iter()
            .flatten()
            {
                if !value.is_array() {
                    return Err(unavailable());
                }
            }
            usable |= model["supported_endpoints"]
                .as_array()
                .is_some_and(|endpoints| {
                    endpoints
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|p| matches!(p, "/v1/messages" | "/responses" | "/chat/completions"))
                });
        }
        if usable { Ok(()) } else { Err(denied()) }
    }
}
