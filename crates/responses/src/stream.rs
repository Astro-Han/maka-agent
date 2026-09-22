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

//! Native Responses dispatch. Protocol code never enters the model V8 isolate.
use crate::{
    decode::Decoder,
    transport::{Exchange, ResponsesLane, Shared},
};
use base64::Engine;
use maka_plugins::http;
use maka_plugins::model::{
    Context, Credentials as ProviderAuth, Events, ProviderKind, Request as ModelRequest,
};
use maka_runtime::model::error::{ModelError, ProviderFailure, ProviderFailureReason};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub async fn stream(
    mut request: ModelRequest,
    context: Context,
    lane: Option<crate::Lane>,
    transport: Arc<Shared>,
) -> Result<(), ModelError> {
    let Context {
        events: sender,
        cancellation,
        idle_timeout,
        transport: network,
    } = context;
    if let Some(lane) = &lane {
        lane.prepare(&mut request);
    }
    let lane = lane.map(|lane| lane.transport);
    let operation = run(
        request,
        sender,
        idle_timeout,
        lane,
        transport,
        cancellation.clone(),
        network,
    );
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(ModelError::Cancelled),
        result = operation => result,
    }
}
async fn wait<T>(duration: Duration, future: impl Future<Output = T>) -> Result<T, ModelError> {
    tokio::time::timeout(duration, future)
        .await
        .map_err(|_| ModelError::TimedOut)
}
async fn emit(
    decoder: &mut Decoder,
    value: Value,
    sender: &Arc<dyn Events>,
) -> Result<(), ModelError> {
    for event in decoder.push(value).map_err(failure)? {
        sender.emit(event).await?;
    }
    Ok(())
}
async fn run(
    request: ModelRequest,
    sender: Arc<dyn Events>,
    idle_timeout: Duration,
    lane: Option<ResponsesLane>,
    transport: Arc<Shared>,
    cancellation: CancellationToken,
    network: Arc<dyn maka_plugins::model::Transport>,
) -> Result<(), ModelError> {
    let plaintext = match request.provider.kind {
        ProviderKind::OpenResponses(p) => Some(p),
        _ => None,
    };
    let mut body = crate::request::Request {
        model: &request.provider.model,
        prompt: &request.prompt,
        tools: &request.tools,
        options: &request.provider_options,
        max_output_tokens: request.max_output_tokens,
        plaintext,
    }
    .encode()
    .map_err(failure)?;
    if let Some(overlay) = &request.provider.body_overlay {
        for (key, value) in overlay {
            if body.get(key).is_some() {
                return Err(ModelError::Adapter(format!(
                    "Extra request body conflicts with a generated field: {key}"
                )));
            }
            body[key] = value.clone();
        }
    }
    let mut url = reqwest::Url::parse(&request.provider.base_url)
        .map_err(|_| ModelError::Adapter("invalid Responses endpoint".into()))?;
    url.set_path(&format!("{}/responses", url.path().trim_end_matches('/')));
    let headers = headers(&request)?;
    let mut decoder = Decoder::new(plaintext, &request.tools);
    if let Some(lane) = lane.filter(|_| {
        request.provider.headers.is_empty()
            && request
                .provider
                .body_overlay
                .as_ref()
                .is_none_or(|value| value.is_empty())
    }) {
        let exchange = Exchange::new(lane, cancellation, network.clone(), transport);
        match wait(
            idle_timeout,
            exchange.start(url.to_string(), headers.clone(), body),
        )
        .await?
        .map_err(failure)?
        {
            Some(full) => body = full,
            None => {
                while let Some(frame) = wait(idle_timeout, exchange.next())
                    .await?
                    .map_err(failure)?
                {
                    sender.progress();
                    let value = serde_json::from_str(&frame).map_err(|_| {
                        ModelError::Adapter("invalid Responses WebSocket JSON".into())
                    })?;
                    emit(&mut decoder, value, &sender).await?;
                }
                return decoder.end().map_err(failure);
            }
        }
    }
    let response = wait(
        idle_timeout,
        network.request(http::Request {
            method: http::Method::Post,
            url: url.to_string(),
            headers: headers.into_iter().collect(),
            body: serde_json::to_vec(&body)
                .map_err(|error| ModelError::Adapter(error.to_string()))?,
        }),
    )
    .await??;
    if !(200..300).contains(&response.head.status) {
        let status = response.head.status;
        let retry_after = header(&response.head, "retry-after")
            .and_then(|v| v.parse::<u64>().ok())
            .and_then(|v| v.checked_mul(1000));
        let mut bytes = Vec::new();
        if header(&response.head, "content-length")
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|len| len > 8 * 1024 * 1024)
        {
            return Err(ModelError::Adapter(
                "provider response body exceeds 8 MiB".into(),
            ));
        }
        while let Some(chunk) = wait(idle_timeout, response.body.next())
            .await?
            .map_err(|_| ModelError::Adapter("Responses error body read failed".into()))?
        {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err(ModelError::Adapter(
                    "provider response body exceeds 8 MiB".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let code = value["error"]["code"]
            .as_str()
            .or_else(|| value["error"]["type"].as_str());
        if code.is_some_and(context_overflow) {
            return Err(ModelError::ContextOverflow {
                observed_output: false,
            });
        }
        let reason = match status {
            429 => Some(ProviderFailureReason::RateLimit),
            500..=599 => Some(ProviderFailureReason::ProviderUnavailable),
            _ => None,
        };
        if let Some(reason) = reason {
            return Err(ModelError::Provider(ProviderFailure::new(
                reason,
                format!("Responses HTTP {status}"),
                true,
                retry_after,
            )));
        }
        return Err(ModelError::Adapter(format!(
            "Responses HTTP {status}: {}",
            value["error"]["message"]
                .as_str()
                .unwrap_or("request rejected")
                .chars()
                .take(4096)
                .collect::<String>()
        )));
    }
    if !header(&response.head, "content-type").is_some_and(|v| {
        v.split(';')
            .next()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"))
    }) {
        return Err(ModelError::Adapter(
            "Responses streaming request did not return SSE".into(),
        ));
    }
    let mut sse = crate::sse::Sse::default();
    while let Some(chunk) = wait(idle_timeout, response.body.next())
        .await?
        .map_err(|error| match error {
            http::Error::Failed(_) => ModelError::Provider(ProviderFailure::new(
                ProviderFailureReason::Network,
                "Responses stream read failed",
                decoder.replay_safe,
                None,
            )),
            http::Error::Denied => ModelError::Cancelled,
            other => ModelError::Adapter(other.to_string()),
        })?
    {
        for byte in chunk {
            if let Some(value) = sse.push(byte).map_err(failure)? {
                emit(&mut decoder, value, &sender).await?;
                if decoder.finished() {
                    return Ok(());
                }
            }
        }
    }
    decoder.end().map_err(failure)
}
fn headers(request: &ModelRequest) -> Result<BTreeMap<String, String>, ModelError> {
    let mut headers = BTreeMap::from([
        ("content-type".into(), "application/json".into()),
        ("accept".into(), "text/event-stream".into()),
    ]);
    let token = match &request.provider.auth {
        ProviderAuth::ApiKey(token) => token,
        ProviderAuth::Codex {
            access_token,
            session_id,
        } => {
            for (key, value) in [
                ("openai-beta", "responses=experimental"),
                ("originator", "codex_cli_rs"),
                ("user-agent", "codex_cli_rs/0.0.0 (Maka)"),
                ("session_id", session_id),
                ("x-client-request-id", session_id),
            ] {
                headers.insert(key.into(), value.into());
            }
            if let Some(account) = account_id(access_token) {
                headers.insert("chatgpt-account-id".into(), account);
            }
            access_token
        }
    };
    headers.insert("authorization".into(), format!("Bearer {token}"));
    for (key, value) in &request.provider.headers {
        let key = key.to_ascii_lowercase();
        if headers.get(&key).is_some_and(|existing| existing != value) {
            return Err(ModelError::Adapter(format!(
                "Custom request header conflicts with a generated header: {key}"
            )));
        }
        headers.insert(key, value.clone());
    }
    Ok(headers)
}
fn account_id(token: &str) -> Option<String> {
    let mut parts = token.split('.');
    parts.next()?;
    let payload = parts.next()?;
    parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    for value in [
        &claims["chatgpt_account_id"],
        &claims["https://api.openai.com/auth"]["chatgpt_account_id"],
    ] {
        if let Some(id) = value.as_str().filter(|v| !v.is_empty()) {
            return Some(id.into());
        }
    }
    claims["organizations"].as_array()?.iter().find_map(|v| {
        v["id"]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    })
}
fn context_overflow(code: &str) -> bool {
    matches!(
        code,
        "context_length_exceeded" | "model_context_window_exceeded" | "request_too_large"
    )
}
fn failure(error: crate::Error) -> ModelError {
    use crate::Error;
    match error {
        Error::Truncated { replay_safe } => ModelError::Provider(ProviderFailure::new(
            ProviderFailureReason::StreamTruncated,
            "model stream ended without finish",
            replay_safe,
            None,
        )),
        Error::Provider {
            code,
            observed_output,
            ..
        } if code.as_deref().is_some_and(context_overflow) => {
            ModelError::ContextOverflow { observed_output }
        }
        Error::Provider {
            code,
            message,
            replay_safe,
            ..
        } if matches!(
            code.as_deref(),
            Some("rate_limit_exceeded" | "server_error" | "service_unavailable")
        ) =>
        {
            let reason = if code.as_deref() == Some("rate_limit_exceeded") {
                ProviderFailureReason::RateLimit
            } else {
                ProviderFailureReason::ProviderUnavailable
            };
            ModelError::Provider(ProviderFailure::new(reason, message, replay_safe, None))
        }
        error => ModelError::Adapter(error.to_string()),
    }
}

fn header<'a>(head: &'a http::Head, name: &str) -> Option<&'a str> {
    head.headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| std::str::from_utf8(value).ok())
}
