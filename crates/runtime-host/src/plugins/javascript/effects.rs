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

use super::invocation::Authority;
use crate::execution::Executions;
use maka_plugins::{fiber::Context, filesystem::Operation};
use maka_runtime::tools::ToolError;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Weak;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub authority: String,
    pub operation: Operation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelRequest {
    pub authority: String,
    pub input: maka_plugins::llm::Generate,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClientRequest {
    pub authority: String,
    pub call: maka_plugins::client_capability::Call,
}
pub(super) enum Effect {
    File(Operation),
    Model(maka_plugins::llm::Generate),
    Client(maka_plugins::client_capability::Call),
}
pub(super) struct Effects {
    host: Weak<Executions>,
    owner: Context,
}
impl Effects {
    pub fn new(host: Weak<Executions>, owner: Context) -> Self {
        Self { host, owner }
    }
    pub async fn clients(&self, authority: Authority) -> Result<Value, ToolError> {
        let host = self.host.upgrade().ok_or_else(|| failed("Host closed"))?;
        let definitions = host
            .plugin_client_catalog(self.owner.clone(), &authority.identity.invocation)
            .await
            .map_err(failed)?;
        serde_json::to_value(definitions).map_err(failed)
    }
    pub async fn invoke(&self, authority: Authority, effect: Effect) -> Result<Value, ToolError> {
        let host = self.host.upgrade().ok_or_else(|| failed("Host closed"))?;
        let owner = self.owner.clone();
        let mut ticket = authority.resources.reserve()?;
        let (send, receive) = tokio::sync::oneshot::channel();
        self.owner
            .spawn_resource("Host SDK effect", move |retiring| async move {
                ticket.start();
                let cancellation = authority.cancellation.child_token();
                let stop = cancellation.clone().drop_guard();
                let operation = async {
                    match effect {
                        Effect::File(operation) => {
                            host.plugin_file(
                                owner,
                                authority.identity.invocation,
                                authority.identity.operation_id,
                                operation,
                                cancellation.clone(),
                            )
                            .await
                            .map_err(failed)?
                            .await
                        }
                        Effect::Model(input) => {
                            host.plugin_model(
                                owner,
                                authority.identity.invocation,
                                authority.identity.operation_id,
                                input,
                                cancellation.clone(),
                            )
                            .await
                            .map_err(failed)?
                            .await
                        }
                        Effect::Client(input) => {
                            host.plugin_client_call(
                                owner,
                                authority.identity.invocation,
                                authority.identity.operation_id,
                                input,
                                cancellation.clone(),
                            )
                            .await
                            .map_err(failed)?
                            .await
                        }
                    }
                };
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
fn failed(error: impl ToString) -> ToolError {
    ToolError::Failed(error.to_string())
}
