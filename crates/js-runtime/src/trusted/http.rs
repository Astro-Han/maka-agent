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

use super::ops::Models;
use deno_core::{JsBuffer, OpState, op2};
use deno_error::JsErrorBox;
use reqwest::{
    Client, Method, Response,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Mutex, OnceLock},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

pub(super) struct Exchange {
    policy: maka_network::Policy,
    client: OnceLock<Client>,
    response: AsyncMutex<Option<Response>>,
    cancellation: CancellationToken,
    current: Mutex<CancellationToken>,
}

impl Exchange {
    pub fn new(policy: maka_network::Policy, cancellation: CancellationToken) -> Self {
        Self {
            policy,
            client: OnceLock::new(),
            response: AsyncMutex::new(None),
            current: Mutex::new(cancellation.child_token()),
            cancellation,
        }
    }

    async fn start(
        &self,
        method: String,
        url: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<Value, JsErrorBox> {
        if body.len() > 32 * 1024 * 1024 {
            return Err(failed("HTTP input exceeds 32 MiB"));
        }
        self.close().await;
        let cancellation = self.cancellation.child_token();
        *self.current.lock().unwrap() = cancellation.clone();
        let mut slot = self.response.lock().await;
        if self.client.get().is_none() {
            let client = self
                .policy
                .client_builder()
                .build()
                .map_err(|_| failed("HTTP client initialization failed"))?;
            let _ = self.client.set(client);
        }
        let method: Method = method.parse().map_err(|_| failed("invalid HTTP method"))?;
        let head = method == Method::HEAD;
        let mut request_headers = HeaderMap::new();
        for (name, value) in headers {
            let name: HeaderName = name.parse().map_err(|_| failed("invalid HTTP header"))?;
            // Fetch Headers use ByteString (Latin-1), not UTF-8.
            let bytes = value
                .chars()
                .map(u8::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| failed("invalid HTTP header"))?;
            let value =
                HeaderValue::from_bytes(&bytes).map_err(|_| failed("invalid HTTP header"))?;
            request_headers.insert(name, value);
        }
        let request = self
            .client
            .get()
            .unwrap()
            .request(method, url)
            .headers(request_headers)
            .body(body);
        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(failed("HTTP request cancelled")),
            response = request.send() => response.map_err(|_| failed("HTTP request failed"))?,
        };
        let status = response.status().as_u16();
        let headers: Vec<_> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                [
                    name.to_string(),
                    value.as_bytes().iter().copied().map(char::from).collect(),
                ]
            })
            .collect();
        let has_body = !head && !matches!(status, 204 | 205 | 304);
        let result = json!({"status":status, "headers":headers, "hasBody":has_body});
        *slot = Some(response);
        Ok(result)
    }

    async fn chunk(&self) -> Result<Option<Vec<u8>>, JsErrorBox> {
        let cancellation = self.current.lock().unwrap().clone();
        let mut slot = self.response.lock().await;
        let Some(response) = slot.as_mut() else {
            return Ok(None);
        };
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(failed("HTTP request cancelled")),
            result = response.chunk() => result.map_err(|_| failed("HTTP response body failed")),
        };
        match result {
            Ok(Some(bytes)) => {
                if bytes.len() > 8 * 1024 * 1024 {
                    slot.take();
                    return Err(failed("HTTP body chunk exceeds 8 MiB"));
                }
                Ok(Some(bytes.to_vec()))
            }
            Ok(None) => {
                slot.take();
                Ok(None)
            }
            Err(error) => {
                slot.take();
                Err(error)
            }
        }
    }

    async fn close(&self) {
        self.current.lock().unwrap().cancel();
        self.response.lock().await.take();
    }
}

fn exchange(state: &OpState, id: u32) -> Result<Rc<Exchange>, JsErrorBox> {
    state
        .borrow::<Models>()
        .active
        .get(&id)
        .map(|model| model.http.clone())
        .ok_or_else(|| failed("HTTP model request closed"))
}
fn failed(message: &'static str) -> JsErrorBox {
    JsErrorBox::generic(message)
}

#[op2]
#[serde]
pub(super) async fn op_http_start(
    state: Rc<RefCell<OpState>>,
    id: u32,
    #[string] method: String,
    #[string] url: String,
    #[serde] headers: BTreeMap<String, String>,
    #[buffer] body: JsBuffer,
) -> Result<serde_json::Value, JsErrorBox> {
    let exchange = exchange(&state.borrow(), id)?;
    exchange.start(method, url, headers, body.to_vec()).await
}

#[op2]
#[buffer]
pub(super) async fn op_http_chunk(
    state: Rc<RefCell<OpState>>,
    id: u32,
) -> Result<Option<Vec<u8>>, JsErrorBox> {
    let exchange = exchange(&state.borrow(), id)?;
    exchange.chunk().await
}

#[op2]
pub(super) async fn op_http_close(state: Rc<RefCell<OpState>>, id: u32) -> Result<(), JsErrorBox> {
    let exchange = exchange(&state.borrow(), id)?;
    exchange.close().await;
    Ok(())
}
