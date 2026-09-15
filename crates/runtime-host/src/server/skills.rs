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

use super::{Host, HostError};
use crate::{execution::skills::FrozenSkills, session::SessionConfiguration};
use maka_protocol::{OperationError, OperationErrorCode as Code, Outcome, skills::*};
use serde_json::Value;
use uuid::Uuid;

mod governance;
mod page;
pub(super) mod sources;
pub(super) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::NotFound,
    Code::SessionArchived,
    Code::PersistenceFailed,
    Code::InternalFailure,
];

pub(super) async fn execute(
    host: &Host,
    connection: Uuid,
    value: &Value,
) -> Result<Outcome, HostError> {
    let input = decode_invocable_input(value)?;
    if host.draining.is_cancelled() {
        return Ok(Outcome::failure(failure(
            Code::HostDraining,
            "Host is draining",
        )));
    }
    let result = query(host, connection, &input).await;
    match result {
        Ok(result) => {
            let output = serde_json::to_value(result)?;
            decode_invocable_output(&output)?;
            Ok(Outcome::success(output))
        }
        Err(error) => Ok(Outcome::failure(error)),
    }
}

async fn query(
    host: &Host,
    connection: Uuid,
    input: &InvocableInput,
) -> Result<InvocableResult, OperationError> {
    let (session_id, cwd, mode, profile) = match input.target() {
        InvocableTarget::Session { session_id } => {
            let session = host
                .log
                .get_session::<SessionConfiguration>(session_id)
                .await
                .map_err(|e| failure(Code::PersistenceFailed, &e.to_string()))?
                .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
            if session.archived {
                return Err(failure(Code::SessionArchived, "Session is archived"));
            }
            if session_id == maka_runtime::workhub::COORDINATION_SESSION_ID {
                // WorkHub never loads Skills. Its invocable catalog is empty by
                // execution policy, not by filesystem discovery or preferences.
                super::workhub::record(host)
                    .await
                    .map_err(|mut error| {
                        if error.code == Code::OperationConflict {
                            error.code = Code::OperationUnavailable;
                        }
                        error
                    })?
                    .ok_or_else(|| failure(Code::NotFound, "WorkHub Session does not exist"))?;
                return page::invocable(
                    input,
                    &session.configuration.workspace.host_cwd,
                    &FrozenSkills {
                        preference_revision: None,
                        discovery: Default::default(),
                        preferences: maka_skills::Preferences::Available(Default::default()),
                        host: Default::default(),
                    },
                );
            }
            let config = session.configuration;
            require_agent(config.collaboration_mode)?;
            (
                Some(session_id.as_str()),
                config.workspace.host_cwd,
                config.permission_mode,
                config.tool_profile,
            )
        }
        InvocableTarget::NewSession {
            context,
            permission_mode,
            collaboration_mode,
        } => {
            require_agent(*collaboration_mode)?;
            let workspace = super::sessions::workspace::resolve(host, &context.workspace)
                .await
                .map_err(|mut error| {
                    // An unavailable project is not a conflicting mutation.
                    if error.code == Code::OperationConflict {
                        error.code = Code::OperationUnavailable;
                    }
                    error
                })?;
            (None, workspace.host_cwd, *permission_mode, None)
        }
    };
    let skills = host
        .executions
        .preview_skills(session_id, connection, &cwd, mode, profile)
        .await?;
    page::invocable(input, &cwd, &skills)
}
fn require_agent(mode: maka_protocol::session::CollaborationMode) -> Result<(), OperationError> {
    if mode == maka_protocol::session::CollaborationMode::Agent {
        Ok(())
    } else {
        Err(failure(
            Code::OperationUnavailable,
            "Plan execution tools are not installed",
        ))
    }
}
fn failure(code: Code, message: &str) -> OperationError {
    OperationError {
        code,
        message: message.into(),
    }
}
