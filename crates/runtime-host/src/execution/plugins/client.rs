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

use super::{Error, Executions, SessionConfiguration, storage};
use maka_plugins::client_capability::Call;
use maka_plugins::fiber::Context;
use maka_runtime::{
    tool_call::{ToolCallIdentity, ToolOrigin},
    tools::{ToolCallContext, ToolDefinition, ToolError, ToolJournal, ToolRegistration},
};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

impl Executions {
    async fn plugin_client_tools(
        &self,
        call: &maka_plugins::call::Scope,
    ) -> Result<Vec<ToolRegistration>, Error> {
        if !self.accepting() {
            return Err(Error::Draining);
        }
        let evidence = self.plugin_agent_evidence(call).await?;
        let frozen = &evidence.invocation;
        let maka_plugins::authorization::Boundary::Session { boundary, .. } = &evidence.boundary
        else {
            return Err(Error::Denied);
        };
        let invocation = call.identity.agent().ok_or(Error::Denied)?;
        let current = self
            .log
            .get_session::<SessionConfiguration>(&invocation.session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        if current.archived
            || current.configuration.boundary_revision != boundary.boundary_revision
            || current.configuration.workspace.host_cwd != frozen.cwd
        {
            return Err(Error::Denied);
        }
        let proof = frozen.tool_composition.as_ref().ok_or(Error::Denied)?;
        let (_, snapshot) = self
            .capabilities
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .restore_bindings(&invocation.session_id, &proof.clients)
            .map_err(|error| Error::Host(error.to_string()))?;
        // Restore only the captured surface, without rebinding the Session or
        // discovering capabilities published after invocation admission.
        Ok(maka_tools::ClientTools::new(
            snapshot,
            self.capabilities.registry.clone(),
            self.capabilities.broker.clone(),
            frozen.cwd.clone(),
            self.interactions.clone(),
        )
        .with_permission_ceiling(boundary.sandbox_mode)
        .registrations()
        .into_iter()
        .filter(|tool| {
            proof
                .bound_tools
                .as_ref()
                .is_none_or(|names| names.contains(&tool.definition.name))
                && current
                    .configuration
                    .bound_tools
                    .as_ref()
                    .is_none_or(|names| names.contains(&tool.definition.name))
        })
        .collect())
    }

    pub(crate) async fn plugin_client_catalog(
        &self,
        owner: Context,
        call: &maka_plugins::call::Scope,
    ) -> Result<Vec<ToolDefinition>, Error> {
        let _lease = owner.admit().map_err(|_| Error::Revoked)?;
        Ok(self
            .plugin_client_tools(call)
            .await?
            .into_iter()
            .map(|tool| tool.definition)
            .collect())
    }

    pub(crate) async fn plugin_client_call(
        self: &Arc<Self>,
        owner: Context,
        call: maka_plugins::call::Scope,
        input: Call,
        cancellation: CancellationToken,
    ) -> Result<impl Future<Output = Result<Value, ToolError>> + Send + 'static, Error> {
        let lease = owner.admit().map_err(|_| Error::Revoked)?;
        let identity = owner.identity().map_err(|_| Error::Revoked)?;
        if cancellation.is_cancelled() {
            return Err(Error::Revoked);
        }
        let registration = self
            .plugin_client_tools(&call)
            .await?
            .into_iter()
            .find(|tool| tool.definition.name == input.name)
            .ok_or(Error::Denied)?;
        let invocation = call.identity.agent().ok_or(Error::Denied)?.clone();
        let parent_operation_id = call.identity.operation_id().map(str::to_owned);
        let operation_id = uuid::Uuid::new_v4().to_string();
        let arguments = Value::Object(input.input);
        // Preparation may wait for provider acceptance or human approval. No Host
        // admission lock is held across it; existing boundary cancellation applies.
        let effect = registration
            .handler
            .prepare(
                input.name.clone(),
                arguments.clone(),
                ToolCallContext {
                    invocation: invocation.clone(),
                    operation_id: operation_id.clone(),
                },
                cancellation.clone(),
            )
            .await
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let admission = self.clone();
        let effect = effect.map_future(move |operation, cancellation| {
            Box::pin(async move {
                let gate = admission.interactions.own_admission().await;
                if cancellation.is_cancelled() {
                    return Err(ToolError::from(Error::Revoked));
                }
                admission
                    .plugin_agent_evidence(&call)
                    .await
                    .map_err(ToolError::from)?;
                drop(gate);
                operation.await
            })
        });
        let journal = ToolJournal::new(self.log.clone(), invocation);
        let host = self.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        self.workers.spawn(async move {
            let result = journal
                .invoke_prepared_call(
                    operation_id,
                    ToolCallIdentity {
                        tool_call_id: uuid::Uuid::new_v4().to_string(),
                        origin: ToolOrigin::HostSdk {
                            package_id: identity.package_id,
                            entry_id: identity.entry_id,
                            activation: identity.activation,
                            parent_operation_id,
                        },
                    },
                    input.name,
                    arguments,
                    cancellation,
                    effect,
                )
                .await;
            if matches!(
                result,
                Err(ToolError::Persistence(_) | ToolError::CleanupUnconfirmed(_))
            ) {
                host.begin_drain();
            }
            drop(lease);
            let _ = send.send(result);
        });
        Ok(async move {
            receive.await.map_err(|_| {
                ToolError::CleanupUnconfirmed("client capability worker disappeared".into())
            })?
        })
    }
}
