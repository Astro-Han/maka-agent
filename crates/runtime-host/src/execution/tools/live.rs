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

use super::*;
use maka_runtime::tool_call::ToolRejection;
use maka_tools::{PreparationFuture, ToolCallContext, ToolPreparer};
use serde_json::Value;
use std::collections::BTreeMap;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(in crate::execution) struct NativeTools {
    pub cwd: String,
    pub profile: Option<maka_protocol::session::SessionToolProfile>,
    pub set: maka_runtime::execution::NativeToolSet,
    pub log: Arc<EventLog>,
    pub writes: Arc<WriteCoordinator>,
    pub shells: Arc<crate::shell::ShellResources>,
    pub controllers: crate::controllers::Controllers,
}

impl NativeTools {
    pub fn registrations(
        &self,
        mode: PermissionMode,
    ) -> Result<Vec<ToolRegistration>, OperationError> {
        super::registrations(self, mode)
    }
}

#[derive(Clone)]
pub(super) struct LiveTools {
    native: Arc<NativeTools>,
    names: Arc<Vec<String>>,
    // Rebuild scoped executors only when the durable mode changes. Definitions
    // are validated by the outer catalog; this cache is not permission authority.
    cached: Arc<Mutex<(PermissionMode, BTreeMap<String, ToolHandler>)>>,
}

impl LiveTools {
    pub fn new(
        native: NativeTools,
        registrations: &[ToolRegistration],
        mode: PermissionMode,
    ) -> Self {
        Self {
            names: Arc::new(
                registrations
                    .iter()
                    .map(|tool| tool.definition.name.clone())
                    .collect(),
            ),
            cached: Arc::new(Mutex::new((mode, handlers(registrations)))),
            native: Arc::new(native),
        }
    }
}

impl ToolPreparer for LiveTools {
    fn names(&self) -> Vec<String> {
        self.names.as_ref().clone()
    }

    fn prepare(
        &self,
        name: String,
        input: Value,
        context: ToolCallContext,
        cancellation: CancellationToken,
    ) -> PreparationFuture {
        let owner = self.clone();
        Box::pin(async move {
            let record = owner
                .native
                .log
                .get_session::<SessionConfiguration>(&context.invocation.session_id)
                .await
                .map_err(rejected)?
                .filter(|record| !record.archived)
                .ok_or_else(|| rejected("Session boundary is unavailable"))?;
            let mode = record.configuration.permission_mode;
            let handler = {
                let mut cached = owner.cached.lock().await;
                if cached.0 != mode {
                    let native = owner.native.clone();
                    let handlers = tokio::task::spawn_blocking(move || {
                        native
                            .registrations(mode)
                            .map(|tools| handlers(&tools))
                            .map_err(|error| rejected(error.message))
                    })
                    .await
                    .map_err(rejected)??;
                    *cached = (mode, handlers);
                }
                cached
                    .1
                    .get(&name)
                    .cloned()
                    .ok_or(ToolRejection::Unavailable)?
            };
            // The returned one-shot effect owns this boundary even if the user
            // widens permissions again before dispatch or during execution.
            handler.prepare(name, input, context, cancellation).await
        })
    }
}

fn handlers(registrations: &[ToolRegistration]) -> BTreeMap<String, ToolHandler> {
    registrations
        .iter()
        .map(|tool| (tool.definition.name.clone(), tool.handler.clone()))
        .collect()
}

fn rejected(error: impl std::fmt::Display) -> ToolRejection {
    ToolRejection::PreparationFailed {
        message: error.to_string(),
    }
}
