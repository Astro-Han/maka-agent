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
    pub workspace_origin: maka_runtime::execution::WorkspaceOrigin,
    pub network_route: maka_network::Policy,
    pub cwd: String,
    pub profile: Option<maka_protocol::session::SessionToolProfile>,
    pub set: maka_runtime::execution::NativeToolSet,
    pub log: Arc<EventLog>,
    pub writes: Arc<WriteCoordinator>,
    pub shells: Arc<crate::shell::ShellResources>,
    pub controllers: crate::controllers::Controllers,
    pub state_root: std::path::PathBuf,
    pub interactions: Arc<crate::server::interactions::Interactions>,
}

impl NativeTools {
    pub fn registrations(
        &self,
        mode: SandboxMode,
        revision: u64,
    ) -> Result<Vec<ToolRegistration>, OperationError> {
        self.registrations_with_grants(mode, revision, &[])
    }

    pub fn registrations_with_grants(
        &self,
        mode: SandboxMode,
        revision: u64,
        grants: &[maka_event_log::interactions::PermissionGrant],
    ) -> Result<Vec<ToolRegistration>, OperationError> {
        super::registrations(self, mode, revision, grants)
    }
}

#[derive(Clone)]
pub(super) struct LiveTools {
    native: Arc<NativeTools>,
    names: Arc<Vec<String>>,
    cached: Arc<Mutex<CachedTools>>,
}

struct CachedTools {
    boundary: Option<(SandboxMode, u64)>,
    handlers: BTreeMap<String, ToolHandler>,
}

impl LiveTools {
    pub fn new(native: NativeTools, registrations: &[ToolRegistration]) -> Self {
        Self {
            names: Arc::new(
                registrations
                    .iter()
                    .map(|tool| tool.definition.name.clone())
                    .collect(),
            ),
            // Catalog preparation supplies definitions, not permission authority.
            cached: Arc::new(Mutex::new(CachedTools {
                boundary: None,
                handlers: BTreeMap::new(),
            })),
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
            let mode = record.configuration.sandbox_mode;
            if record.configuration.workspace_origin != owner.native.workspace_origin
                || record.configuration.workspace.host_cwd != owner.native.cwd
            {
                return Err(rejected("Workspace authority changed"));
            }
            let revision = record.configuration.boundary_revision;
            let mut grants = owner
                .native
                .log
                .permission_grants(&context.invocation, Some(&context.tool_use_id()), revision)
                .await
                .map_err(rejected)?;
            if matches!(name.as_str(), WRITE_NAME | EDIT_NAME | PATCH_NAME)
                && let Some(grant) = owner
                    .native
                    .authorize_write(
                        &name,
                        &input,
                        &context,
                        (mode, revision),
                        &grants,
                        &cancellation,
                    )
                    .await?
            {
                grants.push(grant);
            }
            let handler = if grants.is_empty() {
                let mut cached = owner.cached.lock().await;
                if cached.boundary != Some((mode, revision)) {
                    let native = owner.native.clone();
                    let handlers = tokio::task::spawn_blocking(move || {
                        native
                            .registrations(mode, revision)
                            .map(|tools| handlers(&tools))
                            .map_err(|error| rejected(error.message))
                    })
                    .await
                    .map_err(rejected)??;
                    *cached = CachedTools {
                        boundary: Some((mode, revision)),
                        handlers,
                    };
                }
                cached
                    .handlers
                    .get(&name)
                    .cloned()
                    .ok_or(ToolRejection::Unavailable)?
            } else {
                // Approved additions belong to this call, never to the shared
                // mode cache (especially an approval scoped to one tool use).
                let native = owner.native.clone();
                let name = name.clone();
                tokio::task::spawn_blocking(move || {
                    native
                        .registrations_with_grants(mode, revision, &grants)
                        .map_err(|error| rejected(error.message))?
                        .into_iter()
                        .find(|tool| tool.definition.name == name)
                        .map(|tool| tool.handler)
                        .ok_or(ToolRejection::Unavailable)
                })
                .await
                .map_err(rejected)??
            };
            // The returned one-shot effect owns this boundary even if the user
            // widens permissions again before dispatch or during execution.
            let filesystem = matches!(
                name.as_str(),
                READ_NAME | GLOB_NAME | GREP_NAME | WRITE_NAME | EDIT_NAME | PATCH_NAME
            );
            let session_id = context.invocation.session_id.clone();
            let effect = handler.prepare(name, input, context, cancellation).await?;
            if !filesystem {
                return Ok(effect);
            }
            Ok(effect.map_future(move |effect, cancellation| {
                Box::pin(async move {
                    let admission = tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => return Err(maka_runtime::tools::ToolError::Failed("File operation cancelled before admission".into())),
                        admission = owner.native.interactions.own_admission() => admission,
                    };
                    let current = owner.native.log
                        .get_session::<SessionConfiguration>(&session_id)
                        .await
                        .map_err(|error| maka_runtime::tools::ToolError::Persistence(error.to_string()))?;
                    if !current.is_some_and(|record| {
                        !record.archived && record.configuration.boundary_revision == revision
                    }) {
                        return Err(maka_runtime::tools::ToolError::Failed(
                            "Session permissions changed before file admission".into(),
                        ));
                    }
                    // Admission is ordered against policy changes. Already
                    // accepted file work settles under its captured authority;
                    // scans and disk I/O must not hold the global policy gate.
                    drop(admission);
                    effect.await
                })
            }))
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
