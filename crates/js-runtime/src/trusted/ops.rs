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

use super::{ProviderEvent, Result};
use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

pub(super) struct Output {
    pub http: Rc<super::http::Exchange>,
    pub sender: mpsc::Sender<Result<ProviderEvent>>,
    pub cancellation: CancellationToken,
    pub endpoint: String,
    pub responses: Option<Rc<super::responses::Exchange>>,
}

pub(super) struct Models {
    pub active: HashMap<u32, Output>,
    unsupported_usage: HashSet<String>,
    budget: Arc<Semaphore>,
}

impl Default for Models {
    fn default() -> Self {
        Self {
            active: HashMap::new(),
            unsupported_usage: HashSet::new(),
            budget: Arc::new(Semaphore::new(32 * 1024 * 1024)),
        }
    }
}

#[op2(fast)]
fn op_model_without_stream_usage(state: &mut OpState, id: u32) -> bool {
    let models = state.borrow::<Models>();
    models
        .active
        .get(&id)
        .is_some_and(|output| models.unsupported_usage.contains(&output.endpoint))
}

#[op2(fast)]
fn op_model_reject_stream_usage(state: &mut OpState, id: u32) {
    let models = state.borrow_mut::<Models>();
    if let Some(output) = models.active.get(&id) {
        models.unsupported_usage.insert(output.endpoint.clone());
    }
}

#[op2]
async fn op_model_emit(
    state: Rc<RefCell<OpState>>,
    id: u32,
    #[serde] value: serde_json::Value,
) -> std::result::Result<(), JsErrorBox> {
    let (sender, cancellation, budget) = {
        let state = state.borrow();
        let models = state.borrow::<Models>();
        let output = models
            .active
            .get(&id)
            .ok_or_else(|| JsErrorBox::generic("model request closed"))?;
        (
            output.sender.clone(),
            output.cancellation.clone(),
            models.budget.clone(),
        )
    };
    let bytes = super::budget::bytes(&value, 8 * 1024 * 1024)
        .map_err(|_| JsErrorBox::generic("provider event exceeds 8 MiB"))?;
    let budget = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(JsErrorBox::generic("model cancelled")),
        permit = budget.acquire_many_owned(bytes) =>
            permit.map_err(|_| JsErrorBox::generic("model output budget closed"))?,
    };
    let event = ProviderEvent {
        value,
        _budget: budget,
    };
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(JsErrorBox::generic("model cancelled")),
        result = sender.send(Ok(event)) => result.map_err(|_| JsErrorBox::generic("model receiver closed")),
    }
}

deno_core::extension!(
    maka_trusted,
    ops = [
        op_model_emit,
        op_model_without_stream_usage,
        op_model_reject_stream_usage,
        super::responses_ops::op_responses_start,
        super::responses_ops::op_responses_enabled,
        super::responses_ops::op_responses_next,
        super::responses_ops::op_responses_close,
        super::http::op_http_start,
        super::http::op_http_chunk,
        super::http::op_http_close
    ]
);
