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

use super::{LIMIT, Result, error};
use maka_network::{Policy, Socket};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::{collections::BTreeMap, time::Duration};

/// None permits HTTP fallback: no model request has been dispatched yet.
pub(super) async fn open(
    policy: &Policy,
    url: &str,
    headers: &BTreeMap<String, String>,
) -> Result<Option<Socket>> {
    let mut request_headers = HeaderMap::new();
    for (name, value) in headers {
        if matches!(
            name.as_str(),
            "host" | "connection" | "upgrade" | "content-length"
        ) {
            continue;
        }
        let name: HeaderName = name
            .parse()
            .map_err(|_| error("invalid Responses WebSocket header"))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| error("invalid Responses WebSocket header"))?;
        request_headers.insert(name, value);
    }
    request_headers
        .entry("openai-beta")
        .or_insert(HeaderValue::from_static("responses_websockets=2026-02-06"));
    // One initial attempt, then five exponential retries. The caller's select
    // owns cancellation and the model idle budget, including these sleeps. No
    // response.create has been sent yet, so reconnecting cannot replay it.
    for attempt in 0..=5 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(100 << (attempt - 1))).await;
        }
        if let Ok(Ok(socket)) = tokio::time::timeout(
            Duration::from_secs(15),
            maka_network::connect_websocket(
                policy.client_builder(),
                url,
                request_headers.clone(),
                LIMIT,
            ),
        )
        .await
        {
            return Ok(Some(socket));
        }
    }
    Ok(None)
}
