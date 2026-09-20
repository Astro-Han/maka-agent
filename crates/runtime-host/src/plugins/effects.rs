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

use crate::execution::Executions;
use futures_util::future::BoxFuture;
use maka_plugins::call::Scope as Authority;
use maka_plugins::filesystem::Output;
use maka_plugins::{fiber::Context, filesystem::Operation};
use maka_runtime::tools::ToolError;
use serde_json::Value;
use std::sync::{Arc, Weak};
use tokio_util::sync::CancellationToken;

pub(super) struct Effects {
    host: Weak<Executions>,
    owner: Context,
}
impl Effects {
    pub fn new(host: Weak<Executions>, owner: Context) -> Self {
        Self { host, owner }
    }
    fn host(&self, authority: &Authority) -> Result<Arc<Executions>, ToolError> {
        let host = self.host.upgrade().ok_or_else(|| failed("Host closed"))?;
        if !host.plugin_calls.owns(authority) || authority.cancellation.is_cancelled() {
            return Err(failed("foreign or closed plugin invocation"));
        }
        Ok(host)
    }
    async fn clients(
        &self,
        authority: Authority,
    ) -> Result<Vec<maka_runtime::tools::ToolDefinition>, ToolError> {
        let host = self.host(&authority)?;
        if let Some(invocation) = authority.identity.agent() {
            host.plugin_client_catalog(self.owner.clone(), invocation)
                .await
                .map_err(failed)
        } else {
            host.plugin_resource_client_catalog(self.owner.clone(), &authority)
                .await
        }
    }
    async fn owned<T: Send + 'static>(
        &self,
        authority: Authority,
        effect: impl FnOnce(
            Arc<Executions>,
            Context,
            Authority,
            CancellationToken,
        ) -> BoxFuture<'static, Result<T, ToolError>>
        + Send
        + 'static,
    ) -> Result<T, ToolError> {
        let host = self.host(&authority)?;
        let owner = self.owner.clone();
        let mut ticket = authority.resources.reserve()?;
        let (send, receive) = tokio::sync::oneshot::channel();
        self.owner
            .spawn_resource("Host SDK effect", move |retiring| async move {
                ticket.start();
                let cancellation = authority.cancellation.child_token();
                let stop = cancellation.clone().drop_guard();
                let operation = effect(host, owner, authority, cancellation.clone());
                tokio::pin!(operation);
                let result = tokio::select! {
                    biased;
                    _ = retiring.cancelled() => { cancellation.cancel(); operation.await }
                    result = &mut operation => result,
                };
                let settled = match &result {
                    Err(ToolError::Persistence(error) | ToolError::OutcomeUnknown(error)) => {
                        Err(error.clone())
                    }
                    _ => Ok(()),
                };
                ticket.complete(settled.clone());
                let _ = send.send(result);
                drop(stop);
                settled
            })
            .map_err(failed)?;
        receive
            .await
            .map_err(|_| ToolError::OutcomeUnknown("SDK effect worker disappeared".into()))?
    }
}
impl maka_plugins::filesystem::Files for Effects {
    fn invoke(
        &self,
        call: Authority,
        operation: Operation,
    ) -> BoxFuture<'_, Result<Output, ToolError>> {
        Box::pin(self.owned(call, move |host, owner, call, cancellation| {
            Box::pin(async move {
                if let Some(invocation) = call.identity.agent() {
                    host.plugin_file(
                        owner,
                        invocation.clone(),
                        call.identity.operation_id().map(str::to_owned),
                        operation,
                        cancellation,
                    )
                    .await
                    .map_err(failed)?
                    .await
                    .map(Output::Value)
                } else {
                    host.plugin_resource_file(owner, call, operation, cancellation)
                        .await
                }
            })
        }))
    }
}
impl maka_plugins::llm::Models for Effects {
    fn generate(
        &self,
        call: Authority,
        input: maka_plugins::llm::Generate,
    ) -> BoxFuture<'_, Result<maka_plugins::llm::ModelGeneration, ToolError>> {
        Box::pin(self.owned(call, move |host, owner, call, cancellation| {
            Box::pin(async move {
                if let Some(invocation) = call.identity.agent() {
                    host.plugin_model(
                        owner,
                        invocation.clone(),
                        call.identity.operation_id().map(str::to_owned),
                        input,
                        cancellation,
                    )
                    .await
                    .map_err(failed)?
                    .await
                } else {
                    host.plugin_resource_model(owner, call, input, cancellation)
                        .await
                }
            })
        }))
    }
}
impl maka_plugins::client_capability::Clients for Effects {
    fn tools(
        &self,
        call: Authority,
    ) -> BoxFuture<'_, Result<Vec<maka_runtime::tools::ToolDefinition>, ToolError>> {
        Box::pin(self.clients(call))
    }
    fn call(
        &self,
        call: Authority,
        input: maka_plugins::client_capability::Call,
    ) -> BoxFuture<'_, Result<Value, ToolError>> {
        Box::pin(self.owned(call, move |host, owner, call, cancellation| {
            Box::pin(async move {
                if let Some(invocation) = call.identity.agent() {
                    host.plugin_client_call(
                        owner,
                        invocation.clone(),
                        call.identity.operation_id().map(str::to_owned),
                        input,
                        cancellation,
                    )
                    .await
                    .map_err(failed)?
                    .await
                } else {
                    host.plugin_resource_client(owner, call, input, cancellation)
                        .await
                }
            })
        }))
    }
}
fn failed(error: impl ToString) -> ToolError {
    ToolError::Failed(error.to_string())
}
