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

use super::{Result, failure, invalid, model, mutation, stored};
use crate::{server::Host, session::SessionConfiguration};
use maka_event_log::sessions::SessionExecutionState;
use maka_protocol::{OperationErrorCode as Code, session::*};
use maka_runtime::configuration::Patch;
use serde_json::Value;

pub(super) async fn update(host: &Host, value: &Value) -> Result<SessionUpdateResult> {
    let input = decode_session_configuration_update_input(value).map_err(invalid)?;
    if input.session_id == "maka_workhub_coordination" {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub configuration requires WorkHub authority",
        ));
    }
    apply(host, input).await
}

pub(in crate::server) async fn apply(
    host: &Host,
    input: SessionConfigurationUpdateInput,
) -> Result<SessionUpdateResult> {
    // The same admission lock spans turn.start's configuration capture through
    // durable opening. CAS alone cannot exclude a run prepared from old grants.
    let _admission = host.executions.lock_admission().await;
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let current = host
        .log
        .get_session::<SessionConfiguration>(&input.session_id)
        .await
        .map_err(stored)?
        .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
    if input.session_id == maka_runtime::workhub::COORDINATION_SESSION_ID {
        super::super::workhub::record(host)
            .await?
            .ok_or_else(|| failure(Code::NotFound, "WorkHub Session does not exist"))?;
    }
    if current.revision != input.expected_revision {
        return Ok(SessionUpdateResult::RevisionConflict {
            expected_revision: input.expected_revision,
            actual_revision: current.revision,
        });
    }
    if current.archived {
        return Err(failure(
            Code::OperationConflict,
            "Archived Session configuration cannot be changed",
        ));
    }
    let mut next = merge(host, &current.configuration, input.patch.clone()).await?;
    if next != current.configuration {
        let permission_only = input.patch.permission_mode.is_some()
            && input.patch.model_target.is_none()
            && input.patch.thinking_level.is_keep()
            && input.patch.collaboration_mode.is_none()
            && input.patch.orchestration_mode.is_none();
        let widening = permission_only
            && matches!(
                (current.configuration.permission_mode, next.permission_mode),
                (
                    PermissionMode::Explore,
                    PermissionMode::Ask | PermissionMode::Bypass
                ) | (PermissionMode::Ask, PermissionMode::Bypass)
            );
        if !host
            .log
            .pending_interactions(&input.session_id)
            .await
            .map_err(stored)?
            .is_empty()
        {
            return Err(failure(
                Code::SessionBusy,
                "Session has a pending Interaction",
            ));
        }
        if !widening
            && (current.execution.as_ref().is_some_and(|execution| {
                matches!(execution.state, SessionExecutionState::Live { .. })
            }) || host
                .executions
                .has_session_work(&input.session_id)
                .await
                .map_err(stored)?)
        {
            return Err(failure(
                Code::SessionBusy,
                "Session configuration cannot change while a Turn is active",
            ));
        }
        if next.collaboration_mode != current.configuration.collaboration_mode {
            return Err(failure(
                Code::OperationUnavailable,
                "Collaboration changes require Plan authority",
            ));
        }
        if !widening && next.permission_mode != current.configuration.permission_mode {
            host.executions
                .shells
                .stop_session(&input.session_id)
                .await
                .map_err(|error| failure(Code::PersistenceFailed, &error.to_string()))?;
        }
        if next.permission_mode != PermissionMode::Explore {
            next.labels.retain(|label| label != "mode:deep_research");
        }
    }
    // Even a no-op must recheck CAS after asynchronous model resolution. Every
    // concurrent lifecycle/control/execution change also advances this revision.
    let committed = host
        .log
        .update_session_metadata(
            &input.session_id,
            input.expected_revision,
            move |configuration: &mut SessionConfiguration| {
                *configuration = next;
                Ok(())
            },
        )
        .await
        .map_err(stored)?;
    let output = mutation::result(committed);
    assert_configuration_update_output_for_input(&input, &output).map_err(invalid)?;
    Ok(output)
}

async fn merge(
    host: &Host,
    current: &SessionConfiguration,
    patch: SessionConfigurationPatch,
) -> Result<SessionConfiguration> {
    if current.target.model().is_none()
        && (patch.model_target.is_some()
            || !patch.thinking_level.is_keep()
            || patch
                .orchestration_mode
                .is_some_and(|mode| mode != OrchestrationMode::Default)
            || patch
                .collaboration_mode
                .is_some_and(|mode| mode != CollaborationMode::Agent))
    {
        return Err(failure(
            Code::OperationUnavailable,
            "Executor Sessions do not support native model or orchestration configuration",
        ));
    }
    let mut next = current.clone();
    next.thinking_level = match patch.thinking_level {
        Patch::Keep => current.thinking_level,
        Patch::Clear => None,
        Patch::Set(level) => Some(level),
    };
    if let Some(target) = &patch.model_target {
        next.target = crate::session::SessionTarget::Model {
            model: model::resolve(&host.configuration, target, next.thinking_level).await?,
        };
        next.connection_locked = true;
    } else if !patch.thinking_level.is_keep() {
        let current = current
            .target
            .model()
            .expect("validated model configuration");
        let target = SessionModelTarget::Explicit {
            connection_id: current.connection_id.clone(),
            connection_slug: current.connection_slug.clone(),
            model: current.model.clone(),
        };
        next.target = crate::session::SessionTarget::Model {
            model: model::resolve(&host.configuration, &target, next.thinking_level).await?,
        };
    }
    if let Some(mode) = patch.permission_mode {
        next.permission_mode = mode;
        if mode != current.permission_mode {
            next.boundary_revision = current
                .boundary_revision
                .checked_add(1)
                .filter(|revision| *revision <= 9_007_199_254_740_991)
                .ok_or_else(|| {
                    failure(
                        Code::PersistenceFailed,
                        "Execution boundary revision exhausted",
                    )
                })?;
        }
    }
    if let Some(mode) = patch.collaboration_mode {
        next.collaboration_mode = mode;
    }
    if let Some(mode) = patch.orchestration_mode {
        next.orchestration_mode = mode;
    }
    Ok(next)
}

pub(crate) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::InternalFailure,
    Code::NotFound,
    Code::CommitOutcomeUnknown,
    Code::SessionBusy,
    Code::OperationConflict,
];
