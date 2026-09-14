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

use super::{ops::Models, responses::Exchange};
use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

fn exchange(state: &OpState, id: u32) -> Option<Rc<Exchange>> {
    state.borrow::<Models>().active.get(&id)?.responses.clone()
}

#[op2(fast)]
pub(super) fn op_responses_enabled(state: &mut OpState, id: u32) -> bool {
    exchange(state, id).is_some()
}

#[op2]
#[serde]
pub(super) async fn op_responses_start(
    state: Rc<RefCell<OpState>>,
    id: u32,
    #[string] url: String,
    #[serde] headers: BTreeMap<String, String>,
    #[serde] body: serde_json::Value,
) -> Result<Option<serde_json::Value>, JsErrorBox> {
    let Some(exchange) = exchange(&state.borrow(), id) else {
        return Ok(Some(body));
    };
    exchange.start(url, headers, body).await
}

#[op2]
#[string]
pub(super) async fn op_responses_next(
    state: Rc<RefCell<OpState>>,
    id: u32,
) -> Result<Option<String>, JsErrorBox> {
    let exchange = exchange(&state.borrow(), id)
        .ok_or_else(|| JsErrorBox::generic("Responses request closed"))?;
    exchange.next().await
}

#[op2(fast)]
pub(super) fn op_responses_close(state: &mut OpState, id: u32) {
    if let Some(exchange) = exchange(state, id) {
        exchange.cancel();
    }
}
