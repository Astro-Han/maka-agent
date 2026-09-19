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

use super::super::{Executions, failure};
use crate::plugins::workhub::control::{CandidateFilter, Commands, Result, Stop};
use crate::session::SessionConfiguration;
use futures_util::future::BoxFuture;
use maka_event_log::{
    StoreError,
    workhub::{
        Candidate,
        stop::{StopRecord, StopRequest},
    },
};
use maka_plugins::fiber::Context;
use maka_protocol::OperationErrorCode as Code;
use std::sync::{Arc, Weak};

pub(crate) struct WorkHubCommands {
    executions: Weak<Executions>,
    pub(super) project_usage: crate::server::ProjectUsage,
}
impl WorkHubCommands {
    fn executions(&self) -> Result<Arc<Executions>> {
        self.executions
            .upgrade()
            .ok_or_else(|| failure(Code::HostDraining, "Host is closed"))
    }

    pub(crate) async fn recover(&self) -> Result<()> {
        super::correction::recover(self, &self.executions()?).await
    }

    pub(super) async fn validate_target(
        &self,
        executions: &Executions,
        target: &crate::plugins::workhub::target::Target,
    ) -> Result<()> {
        if let crate::plugins::workhub::target::Target::Created { creation, .. } = target {
            executions.validate_creation(creation).await?;
            self.project_usage
                .record(&creation.configuration.workspace)
                .await?;
        }
        Ok(())
    }

    pub(crate) fn new(
        executions: &Arc<Executions>,
        project_usage: crate::server::ProjectUsage,
    ) -> Self {
        Self {
            executions: Arc::downgrade(executions),
            project_usage,
        }
    }
}
impl Commands for WorkHubCommands {
    fn correction(
        &self,
        caller: Context,
        action_id: maka_runtime::workhub::ActionId,
        turn_id: String,
    ) -> BoxFuture<'_, Result<Option<maka_event_log::workhub::correction::CorrectionRecord>>> {
        Box::pin(async move {
            super::correction::probe(&self.executions()?, caller, action_id, turn_id).await
        })
    }
    fn correct(
        &self,
        caller: Context,
        request: crate::plugins::workhub::correction::Request,
    ) -> BoxFuture<'_, Result<maka_event_log::workhub::correction::CorrectionRecord>> {
        Box::pin(async move {
            super::correction::execute(self, &self.executions()?, caller, request).await
        })
    }
    fn settle_correction(
        &self,
        identity: crate::plugins::workhub::control::Identity,
    ) -> BoxFuture<'_, Result<maka_event_log::workhub::correction::CorrectionRecord>> {
        Box::pin(
            async move { super::correction::settle(self, &self.executions()?, identity).await },
        )
    }
    fn delegation(
        &self,
        caller: Context,
        identity: crate::plugins::workhub::control::Identity,
    ) -> BoxFuture<'_, Result<Option<maka_runtime::workhub::Delegation>>> {
        Box::pin(async move {
            let executions = self.executions()?;
            super::delegation::probe(&executions, caller, identity).await
        })
    }
    fn delegate(
        &self,
        caller: Context,
        request: crate::plugins::workhub::delegation::Request,
    ) -> BoxFuture<'_, Result<maka_runtime::workhub::Delegation>> {
        Box::pin(async move {
            let executions = self.executions()?;
            super::delegation::execute(self, &executions, caller, request).await
        })
    }
    fn target(
        &self,
        session: String,
        eligible: CandidateFilter,
    ) -> BoxFuture<'_, Result<Option<maka_event_log::sessions::SessionRecord<SessionConfiguration>>>>
    {
        Box::pin(async move {
            let executions = self.executions()?;
            executions.workhub_target(&session, eligible).await
        })
    }
    fn prepare_session(
        &self,
        request: maka_protocol::session::SessionCreateInput,
    ) -> BoxFuture<'_, Result<crate::execution::Creation>> {
        Box::pin(async move {
            let executions = self.executions()?;
            executions.prepare_session(request).await
        })
    }
    fn resume(
        &self,
        caller: Context,
        request: crate::plugins::workhub::resume::Request,
        connection: uuid::Uuid,
        eligible: CandidateFilter,
    ) -> BoxFuture<'_, Result<crate::plugins::workhub::resume::Receipt>> {
        Box::pin(async move {
            let executions = self.executions()?;
            super::resume::execute(&executions, caller, request, connection, eligible).await
        })
    }
    fn candidates(
        &self,
        eligible: CandidateFilter,
    ) -> BoxFuture<'_, Result<Vec<Candidate<SessionConfiguration>>>> {
        Box::pin(async move {
            let executions = self.executions()?;
            if executions.shutdown.is_cancelled() {
                return Err(failure(Code::HostDraining, "Host is draining"));
            }
            executions
                .log
                .workhub_candidates(eligible)
                .await
                .map_err(|error| stored(&executions, error))
        })
    }
    fn stop(&self, caller: Context, request: Stop) -> BoxFuture<'_, Result<StopRecord>> {
        Box::pin(async move {
            let executions = self.executions()?;
            let (record, completed, _call) = {
                let _gate = executions.lock_admission().await;
                let call = caller
                    .admit()
                    .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
                let record = if let Some(record) = executions
                    .log
                    .workhub_stop(&request.action_id)
                    .await
                    .map_err(|error| stored(&executions, error))?
                {
                    if record.intent.request.source.turn_id != request.turn_id
                        || record.intent.request.request_fingerprint != request.request_fingerprint
                        || record.intent.request.target_session_id != request.target_session_id
                    {
                        return Err(failure(
                            Code::OperationConflict,
                            "WorkHub stop belongs to another request",
                        ));
                    }
                    record
                } else {
                    if executions.shutdown.is_cancelled() {
                        return Err(failure(Code::HostDraining, "Host is draining"));
                    }
                    let source = executions.workhub_source(&request.turn_id).await?;
                    executions
                        .log
                        .request_workhub_stop(StopRequest {
                            action_id: request.action_id,
                            request_fingerprint: request.request_fingerprint,
                            source: source.invocation,
                            target_session_id: request.target_session_id,
                        })
                        .await
                        .map_err(|error| stored(&executions, error))?
                };
                if record.resolution.is_some() {
                    return Ok(record);
                }
                if executions.shutdown.is_cancelled() {
                    return Err(failure(Code::HostDraining, "Host is draining"));
                }
                let completed = executions
                    .stop_workhub_owner(
                        record.intent.owner.as_ref().ok_or_else(|| {
                            failure(Code::InternalFailure, "Stop intent has no owner")
                        })?,
                        &record.intent.request.action_id,
                    )
                    .await?;
                (record, completed, call)
            };
            if let Some(completed) = completed {
                completed.cancelled().await;
            }
            let resolved = executions
                .log
                .resolve_workhub_stop(&record.intent.request.action_id)
                .await
                .map_err(|error| {
                    if matches!(error, StoreError::SessionBusy) {
                        failure(
                            Code::OperationUnavailable,
                            "WorkHub stop owner is still recovering",
                        )
                    } else {
                        stored(&executions, error)
                    }
                })?;
            let mut admission = Some(executions.lock_admission().await);
            executions
                .dispatch_pending(&record.intent.request.target_session_id, &mut admission)
                .await?;
            Ok(resolved)
        })
    }
}

pub(super) fn stored(executions: &Executions, error: StoreError) -> maka_protocol::OperationError {
    let code = match &error {
        StoreError::InvalidTransition(_) | StoreError::SessionConflict => Code::OperationConflict,
        StoreError::SessionNotFound => Code::NotFound,
        StoreError::SessionBusy => Code::SessionBusy,
        StoreError::CommitUnknown(_) | StoreError::OperationUnknown => {
            executions.begin_drain();
            Code::CommitOutcomeUnknown
        }
        _ => Code::PersistenceFailed,
    };
    failure(code, &error.to_string())
}
