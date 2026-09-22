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

use super::{Error, Executions, SessionConfiguration};
use maka_plugins::{authorization::Boundary, call, execution::SessionBoundary};
use maka_runtime::{execution::InvocationConfiguration, tools::ToolError};
use std::sync::Arc;

pub(in crate::execution) struct AgentAdmission {
    pub log: Arc<maka_event_log::EventLog>,
}

pub(super) struct AgentEvidence {
    pub invocation: InvocationConfiguration,
    pub boundary: Boundary,
}

impl call::Admission for AgentAdmission {
    fn authorize<'a>(
        &'a self,
        identity: &'a call::Identity,
    ) -> futures_util::future::BoxFuture<'a, Result<call::Evidence, ToolError>> {
        Box::pin(async move {
            let invocation = identity
                .agent()
                .ok_or_else(|| denied("not an Agent call"))?;
            let frozen = self
                .log
                .invocation_configuration(invocation)
                .await
                .map_err(|error| ToolError::Persistence(error.to_string()))?
                .ok_or_else(|| denied("invocation is unavailable"))?;
            let current = self
                .log
                .get_session::<SessionConfiguration>(&invocation.session_id)
                .await
                .map_err(|error| ToolError::Persistence(error.to_string()))?
                .filter(|session| !session.archived)
                .ok_or_else(|| denied("Session is unavailable"))?
                .configuration;
            // Permission changes affect new calls, not an already issued scope.
            // Workspace identity and the admitted tool surface remain Run facts.
            if current.workspace.host_cwd != frozen.cwd
                || current.workspace_origin != frozen.workspace_origin
            {
                return Err(denied("workspace authority changed"));
            }
            let boundary = Boundary::Session {
                boundary: SessionBoundary {
                    session_id: invocation.session_id.clone(),
                    cwd: current.workspace.host_cwd,
                    workspace_origin: current.workspace_origin,
                    boundary_revision: current.boundary_revision,
                    sandbox_mode: current.sandbox_mode,
                    approval_policy: current.approval_policy,
                },
                workspace_identity: frozen
                    .workspace_identity
                    .clone()
                    .ok_or_else(|| denied("workspace identity is unavailable"))?,
            };
            crate::server::plugin_authorization::validate_boundary(&self.log, &boundary)
                .await
                .map_err(|error| denied(&error.message))?;
            Ok(Arc::new(AgentEvidence {
                invocation: frozen,
                boundary,
            }) as call::Evidence)
        })
    }
}

impl Executions {
    pub(super) async fn plugin_agent_evidence<'a>(
        &self,
        call: &'a call::Scope,
    ) -> Result<&'a AgentEvidence, Error> {
        if !self.accepting() || call.cancellation.is_cancelled() || call.identity.agent().is_none()
        {
            return Err(Error::Revoked);
        }
        let evidence = self
            .plugin_calls
            .evidence::<AgentEvidence>(call)
            .ok_or(Error::Denied)?;
        crate::server::plugin_authorization::validate_boundary(&self.log, &evidence.boundary)
            .await
            .map_err(super::authority::consent_error)?;
        Ok(evidence)
    }
}

fn denied(reason: &str) -> ToolError {
    ToolError::Failed(format!("Plugin call denied: {reason}"))
}
