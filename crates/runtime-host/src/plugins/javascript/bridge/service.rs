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
    State,
    wire::{self, Error},
};
use crate::plugins::javascript::{callbacks, invocation::Authority};
use futures_util::future::BoxFuture;
use maka_plugins::services::method::{self, Handle, Method};
use serde_json::{Value, json};
use std::sync::Arc;
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
        let stopping = self.context.lifecycle.stopping()?;
        invoke(reference, input.input, authority, stopping).await
    }
}
async fn invoke(
    reference: Handle,
    input: Value,
    authority: Option<Authority>,
    stopping: CancellationToken,
) -> Result<Value, Error> {
    reference
        .call(input, authority, stopping)
        .await
        .map_err(|error| Error {
            code: match error {
                method::Error::Retired => wire::Code::Revoked,
                method::Error::Invalid(_) => wire::Code::Invalid,
                method::Error::Failed(_) => wire::Code::Unavailable,
                method::Error::OutcomeUnknown(_) => wire::Code::OutcomeUnknown,
            },
            message: error.to_string(),
        })
}

pub(super) struct JavaScript {
    pub callback: Arc<callbacks::Callback>,
}
impl Method<Value, Value> for JavaScript {
    fn call(
        &self,
        input: Value,
        context: method::Context,
    ) -> BoxFuture<'_, Result<Value, method::Error>> {
        Box::pin(async move {
            let forwarded = context
                .invocation
                .map(|authority| self.callback.calls.forward(authority))
                .transpose()
                .map_err(|error| method::Error::Failed(error.to_string()))?;
            let cancellation = context.cancellation.child_token();
            let stop = cancellation.clone().drop_guard();
            let source = async {
                match &forwarded {
                    Some(call) => call.authority.cancellation.cancelled().await,
                    None => std::future::pending().await,
                }
            };
            let context = match &forwarded {
                Some(call) => json!({"configuration":context.configuration, "authority":call.id,
            "source":call.authority.identity, "invocation":call.authority.identity.agent(), "operationId":call.authority.identity.operation_id()}),
                None => json!({"configuration":context.configuration}),
            };
            let invocation = callbacks::invoke(
                &self.callback.module,
                self.callback.id,
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
            result.map_err(|error| match error {
                error @ (maka_runtime::tools::ToolError::Failed(_)
                | maka_runtime::tools::ToolError::Io { .. }) => {
                    method::Error::Failed(error.to_string())
                }
                error => method::Error::OutcomeUnknown(error.to_string()),
            })
        })
    }
}
