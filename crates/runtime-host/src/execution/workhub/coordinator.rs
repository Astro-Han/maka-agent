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

use super::{
    super::{Executions, Result, failure, internal},
    commands::stored,
};
use crate::{
    plugins::workhub::coordinator::{
        Resolution, fingerprint, validate, validate_configuration, validate_workspace,
    },
    session::SessionConfiguration,
};
use maka_event_log::sessions::{SessionExecutionState, SessionMutation, SessionRecord};
use maka_plugins::fiber::Context;
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::workhub::COORDINATION_SESSION_ID;
use std::sync::Arc;

mod model;
pub(super) use model::configure;

impl Executions {
    /// Identity validation remains available for already accepted core work,
    /// independent of a currently published business implementation.
    pub(crate) async fn workhub_coordinator(
        &self,
    ) -> Result<Option<SessionRecord<SessionConfiguration>>> {
        let record = self
            .log
            .probe_session_create(COORDINATION_SESSION_ID, &fingerprint())
            .await
            .map_err(|error| stored(self, error))?;
        if let Some(record) = &record {
            validate(record)?;
        }
        Ok(record)
    }
}

pub(super) async fn resolve(
    executions: &Arc<Executions>,
    caller: Context,
    resolution: Resolution,
) -> Result<()> {
    let _gate = executions.lock_admission().await;
    let _call = caller
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    if executions.shutdown.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    validate_workspace(&resolution.workspace)?;
    if let Some(record) = executions.workhub_coordinator().await? {
        if record.configuration.workspace == resolution.workspace {
            return Ok(());
        }
        if executions
            .has_session_work(COORDINATION_SESSION_ID)
            .await
            .map_err(|error| stored(executions, error))?
            || record.execution.as_ref().is_some_and(|execution| {
                matches!(execution.state, SessionExecutionState::Live { .. })
            })
        {
            return Err(failure(
                Code::OperationUnavailable,
                "WorkHub workspace cannot change while executing",
            ));
        }
        let result = executions
            .log
            .update_session_metadata(
                COORDINATION_SESSION_ID,
                record.revision,
                move |configuration: &mut SessionConfiguration| {
                    configuration.workspace = resolution.workspace;
                    Ok(())
                },
            )
            .await
            .map_err(|error| stored(executions, error))?;
        if matches!(result, SessionMutation::RevisionConflict { .. }) {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub Session changed during resolution",
            ));
        }
        return Ok(());
    }
    let creation = resolution.creation.ok_or_else(|| {
        failure(
            Code::OperationConflict,
            "WorkHub Session changed during preparation",
        )
    })?;
    if creation.configuration.workspace != resolution.workspace {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub workspace changed during preparation",
        ));
    }
    executions.validate_creation(&creation).await?;
    validate_configuration(&creation.configuration)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(internal)?
        .as_millis()
        .try_into()
        .map_err(internal)?;
    let record = executions
        .log
        .create_session(
            COORDINATION_SESSION_ID,
            &fingerprint(),
            &creation.configuration,
            now,
        )
        .await
        .map_err(|error| stored(executions, error))?;
    validate(&record)
}
