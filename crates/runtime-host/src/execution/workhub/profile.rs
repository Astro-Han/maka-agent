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

use super::{Executions, Result, SessionConfiguration, failure};
use crate::execution::read;
use crate::plugins::workhub::{ID, Policy};
use maka_client_capability::BindingError;
use maka_plugins::{composition::Scope, contributions::Contribution};
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::workhub::COORDINATION_SESSION_ID;
use maka_tools::{
    ToolCatalog, ToolDefinition, ToolHandler, ToolNesting, ToolRegistration, ToolSemantics,
};
use std::sync::Arc;
use uuid::Uuid;

pub(super) fn tools(
    executions: &Executions,
    session: &SessionConfiguration,
    connection_id: Uuid,
    policy: &Policy,
) -> Result<(ToolCatalog, maka_runtime::execution::ToolComposition)> {
    let (tools, clients) = executions
        .capabilities
        .bind_required_tools(
            COORDINATION_SESSION_ID,
            connection_id,
            policy.required_clients,
            policy.optional_clients,
            session.workspace.host_cwd.clone(),
            executions.interactions.clone(),
        )
        .map_err(|error| {
            failure(
                if matches!(error, BindingError::Draining) {
                    Code::HostDraining
                } else if matches!(error, BindingError::RequiredProvider) {
                    Code::OperationUnavailable
                } else {
                    Code::OperationConflict
                },
                &error.to_string(),
            )
        })?;
    Ok((
        bound_catalog(executions, tools, policy)?,
        maka_runtime::execution::ToolComposition {
            clients,
            bound_tools: None,
            skills_digest: None,
        },
    ))
}

pub(in crate::execution) fn catalog(
    executions: &Executions,
    tools: Vec<ToolRegistration>,
) -> Result<ToolCatalog> {
    let policy = resolve(executions)?;
    let _admission = policy
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    bound_catalog(executions, tools, &policy.value)
}

pub(super) fn resolve(executions: &Executions) -> Result<Contribution<Policy>> {
    executions
        .plugin_catalog
        .snapshot::<Policy>(&Scope::Profile)
        .entries
        .remove(ID)
        .ok_or_else(|| failure(Code::OperationUnavailable, "WorkHub policy is unavailable"))
}

fn bound_catalog(
    executions: &Executions,
    mut tools: Vec<ToolRegistration>,
    policy: &Policy,
) -> Result<ToolCatalog> {
    tools.retain(|tool| policy.allows_client(&tool.definition.name));
    tools.push(executions.interactions.question_tool());
    tools.push(ToolRegistration {
        definition: ToolDefinition {
            name: maka_fs_tools::READ_NAME.into(),
            description: policy.attachment_description.into(),
            input_schema: read::schema(),
        },
        handler: ToolHandler::Prepared(Arc::new(read::SessionRead::attachments(
            executions.log.clone(),
        ))),
        nesting: ToolNesting::Nestable,
        semantics: ToolSemantics::Parallel,
    });
    ToolCatalog::new(tools).map_err(|e| failure(Code::OperationUnavailable, &e.to_string()))
}
