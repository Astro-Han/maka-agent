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
use maka_plugins::{http, model::Transport};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

pub(super) struct Exchange {
    network: Arc<dyn Transport>,
    response: AsyncMutex<Option<http::Response>>,
    cancellation: CancellationToken,
    current: Mutex<CancellationToken>,
}
impl Exchange {
    pub fn new(network: Arc<dyn Transport>, cancellation: CancellationToken) -> Self {
        Self {
            network,
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
        self.close().await;
        let method: http::Method =
            serde_json::from_value(json!(method)).map_err(|_| failed("invalid HTTP method"))?;
        let head = matches!(method, http::Method::Head);
        let cancellation = self.cancellation.child_token();
        *self.current.lock().unwrap() = cancellation.clone();
        let mut slot = self.response.lock().await;
        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(failed("HTTP request cancelled")),
            response = self.network.request(http::Request { method, url, headers: headers.into_iter().collect(), body }) =>
                response.map_err(|error| match error {
                    maka_plugins::model::Error::Provider(_) => JsErrorBox::from_err(TransportFailure),
                    other => JsErrorBox::generic(other.to_string()),
                })?,
        };
        let status = response.head.status;
        let headers: Vec<_> = response
            .head
            .headers
            .iter()
            .map(|(name, value)| {
                [
                    name.clone(),
                    value.iter().copied().map(char::from).collect(),
                ]
            })
            .collect();
        let has_body = !head && !matches!(status, 204 | 205 | 304);
        let result = json!({"status": status, "headers": headers, "hasBody": has_body});
        *slot = Some(response);
        Ok(result)
    }
    async fn chunk(&self) -> Result<Option<Vec<u8>>, JsErrorBox> {
        let cancellation = self.current.lock().unwrap().clone();
        let mut slot = self.response.lock().await;
        let Some(response) = slot.as_ref() else {
            return Ok(None);
        };
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(failed("HTTP request cancelled")),
            result = response.body.next() => result.map_err(|error| match error {
                http::Error::Failed(_) => JsErrorBox::from_err(TransportFailure),
                other => JsErrorBox::generic(other.to_string()),
            }),
        };
        if !matches!(result, Ok(Some(_))) {
            slot.take();
        }
        result
    }
    async fn close(&self) {
        self.current.lock().unwrap().cancel();
        if let Some(response) = self.response.lock().await.take() {
            let _ = response.body.close().await;
        }
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
#[derive(Debug, thiserror::Error, deno_error::JsError)]
#[class(generic)]
#[property("code" = "MAKA_HTTP_TRANSPORT")]
#[error("model HTTP transport interrupted")]
struct TransportFailure;

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
