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

use super::{Executions, Operation, Prepared, failed};
use futures_util::future::BoxFuture;
use maka_plugins::{authorization::Capability, call::Scope, fiber::Context};
use maka_runtime::{
    tool_call::{ToolCallIdentity, ToolOrigin},
    tools::{PreparedEffect, ToolError, ToolJournal},
};
use serde_json::Value;

impl Executions {
    /// Streaming owns its journal until EOF or interruption, not just until
    /// response headers arrive. The response worker survives a dropped caller.
    pub(crate) async fn record_plugin_http(
        &self,
        owner: Context,
        call: Scope,
        operation: Operation,
        destination: maka_sandbox::Destination,
        effect: BoxFuture<'static, Result<Value, ToolError>>,
    ) -> Result<Value, ToolError> {
        let cancellation = call.cancellation.clone();
        let Some(invocation) = call.identity.agent() else {
            return self
                .journal(
                    owner,
                    call,
                    Prepared {
                        operation,
                        capability: Capability::Network,
                        effect,
                    },
                    cancellation,
                )
                .await;
        };
        let _lease = owner.admit().map_err(failed)?;
        let identity = owner.identity().map_err(failed)?;
        let gate = self
            .admit_plugin_network(&call, &destination)
            .await
            .map_err(ToolError::from)?;
        let input = serde_json::to_value(operation).map_err(failed)?;
        let result = ToolJournal::new(self.log.clone(), invocation.clone())
            .invoke_prepared_call(
                uuid::Uuid::new_v4().to_string(),
                ToolCallIdentity {
                    tool_call_id: uuid::Uuid::new_v4().to_string(),
                    origin: ToolOrigin::HostSdk {
                        package_id: identity.package_id,
                        entry_id: identity.entry_id,
                        activation: identity.activation,
                        parent_operation_id: call.identity.operation_id().map(str::to_owned),
                    },
                },
                "Http".into(),
                input,
                cancellation,
                PreparedEffect::new(move |_| {
                    Box::pin(async move {
                        // T1 is durable; do not hold global admission behind I/O.
                        drop(gate);
                        effect.await.map(Into::into)
                    })
                }),
            )
            .await;
        if matches!(
            result,
            Err(ToolError::Persistence(_) | ToolError::CleanupUnconfirmed(_))
        ) {
            self.begin_drain();
        }
        result
    }
}
