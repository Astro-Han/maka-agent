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

use super::{
    Reference, State,
    wire::{self, Error},
};
use crate::plugins::javascript::{callbacks, invocation::Authority};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

impl State {
    pub(super) async fn call_service(&self, input: wire::Call) -> Result<Value, Error> {
        let reference = self
            .handles
            .lock()
            .unwrap()
            .get(&input.handle)
            .cloned()
            .ok_or(maka_plugins::Error::Retired)?;
        let authority = input
            .authority
            .as_deref()
            .map(|id| self.calls.get(id))
            .transpose()?;
        let Some(authority) = authority else {
            let stopping = self.context.lifecycle.stopping()?;
            return invoke(reference, input.input, None, stopping).await;
        };
        // A forwarded invocation is a child resource, not just a JS promise.
        // Its own resource group closes independently and reports into its parent.
        let mut ticket = authority.resources.reserve().map_err(Error::invalid)?;
        let (send, receive) = tokio::sync::oneshot::channel();
        self.context
            .lifecycle
            .spawn_resource("service call", move |retiring| async move {
                ticket.start();
                let result = invoke(reference, input.input, Some(authority), retiring).await;
                let settled = match &result {
                    Err(error) if matches!(error.code, wire::Code::OutcomeUnknown) => {
                        Err(error.message.clone())
                    }
                    _ => Ok(()),
                };
                ticket.complete(settled.clone());
                let _ = send.send(result);
                settled
            })?;
        receive.await.map_err(uncertain)?
    }
}
async fn invoke(
    reference: Reference,
    input: Value,
    authority: Option<Authority>,
    stopping: CancellationToken,
) -> Result<Value, Error> {
    let configuration = reference.configuration;
    let service = reference.service.acquire()?;
    let forwarded = authority
        .map(|authority| {
            service
                .callback
                .calls
                .enter(authority.identity, authority.cancellation)
        })
        .transpose()
        .map_err(Error::invalid)?;
    let cancellation = stopping.child_token();
    let stop = cancellation.clone().drop_guard();
    let source = async {
        match &forwarded {
            Some(call) => call.authority.cancellation.cancelled().await,
            None => std::future::pending().await,
        }
    };
    let context = match &forwarded {
        Some(call) => json!({"configuration":configuration, "authority":call.id,
            "invocation":call.authority.identity.invocation, "operationId":call.authority.identity.operation_id}),
        None => json!({"configuration":configuration}),
    };
    let invocation = callbacks::invoke(
        &service.callback.module,
        service.callback.id,
        input,
        context,
        cancellation.clone(),
    );
    tokio::pin!(invocation);
    let result = tokio::select! {
        result = &mut invocation => result,
        _ = source => { cancellation.cancel(); invocation.await },
    };
    drop(stop);
    if let Some(forwarded) = &forwarded {
        forwarded.finish().await.map_err(uncertain)?;
    }
    result.map_err(|error| Error {
        code: match error {
            maka_runtime::tools::ToolError::Failed(_) => wire::Code::Unavailable,
            _ => wire::Code::OutcomeUnknown,
        },
        message: error.to_string(),
    })
}
fn uncertain(error: impl ToString) -> Error {
    Error {
        code: wire::Code::OutcomeUnknown,
        message: error.to_string(),
    }
}
