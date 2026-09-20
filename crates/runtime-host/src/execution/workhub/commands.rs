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
    workhub::{Candidate, stop::StopRecord},
};
use maka_plugins::fiber::Context;
use maka_protocol::OperationErrorCode as Code;
use std::sync::{Arc, Weak};

pub(crate) struct WorkHubCommands {
    executions: Weak<Executions>,
    pub(super) project_usage: crate::server::ProjectUsage,
    root_id: String,
    epoch: String,
    catalog: Arc<crate::server::CatalogFeed>,
    changes: tokio::sync::broadcast::Sender<serde_json::Value>,
}
impl WorkHubCommands {
    fn executions(&self) -> Result<Arc<Executions>> {
        self.executions
            .upgrade()
            .ok_or_else(|| failure(Code::HostDraining, "Host is closed"))
    }

    pub(crate) async fn recover(&self) -> Result<()> {
        super::correction::recover(&self.executions()?).await
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
        root_id: String,
        epoch: String,
        catalog: Arc<crate::server::CatalogFeed>,
        changes: tokio::sync::broadcast::Sender<serde_json::Value>,
    ) -> Self {
        Self {
            executions: Arc::downgrade(executions),
            project_usage,
            root_id,
            epoch,
            catalog,
            changes,
        }
    }

    async fn coordinator_changed(&self) -> Result<()> {
        self.catalog
            .publish_session(
                &self.changes,
                maka_runtime::workhub::COORDINATION_SESSION_ID,
            )
            .await
            .map_err(super::super::internal)
    }
}
impl Commands for WorkHubCommands {
    fn enqueue(
        &self,
        caller: Context,
        input: maka_protocol::message::SubmitInput,
        expected_turn: String,
        connection: uuid::Uuid,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'_, Result<maka_protocol::message::SubmitResult>> {
        Box::pin(async move {
            self.executions()?
                .enqueue_managed(
                    input,
                    connection,
                    &self.root_id,
                    &self.epoch,
                    crate::execution::message::Manager {
                        owner: caller,
                        expected_turn,
                        cancellation,
                    },
                )
                .await
        })
    }
    fn message_observation(
        &self,
        session: String,
        message: String,
    ) -> BoxFuture<'_, Result<maka_event_log::observation::MessageObservation>> {
        Box::pin(async move {
            let executions = self.executions()?;
            executions
                .log
                .message_observation(&session, &message)
                .await
                .map_err(|error| stored(&executions, error))
        })
    }
    fn answer_receipt<'a>(
        &'a self,
        request: &'a crate::plugins::workhub::answer::Request,
    ) -> BoxFuture<'a, Result<Option<maka_protocol::turn::TurnSnapshot>>> {
        Box::pin(async move { self.executions()?.workhub_answer_receipt(request).await })
    }
    fn client_call(
        &self,
        caller: Context,
        context: maka_runtime::tools::ToolCallContext,
        input: maka_plugins::client_capability::Call,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'_, std::result::Result<serde_json::Value, maka_runtime::tools::ToolError>> {
        Box::pin(async move {
            let failed = |error: String| maka_runtime::tools::ToolError::Failed(error);
            self.executions()
                .map_err(|error| failed(error.message))?
                .plugin_client_call(
                    caller,
                    context.invocation,
                    Some(context.operation_id),
                    input,
                    cancellation,
                )
                .await
                .map_err(|error| failed(error.to_string()))?
                .await
        })
    }
    fn client_connection(
        &self,
        caller: Context,
        invocation: maka_runtime::event::Invocation,
        tool: &'static str,
    ) -> BoxFuture<'_, Result<uuid::Uuid>> {
        Box::pin(async move {
            self.executions()?
                .plugin_client_connection(caller, invocation, tool)
                .await
                .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))
        })
    }
    fn chat_defaults(
        &self,
    ) -> BoxFuture<'_, Result<maka_runtime::configuration::policy::ChatDefaults>> {
        Box::pin(async move {
            self.executions()?
                .configuration
                .runtime_policy()
                .await
                .map(|snapshot| snapshot.policy.chat_defaults)
                .map_err(super::super::internal)
        })
    }
    fn answer(
        &self,
        caller: Context,
        plan: crate::plugins::workhub::answer::Plan,
        connection: uuid::Uuid,
    ) -> BoxFuture<'_, Result<maka_protocol::workhub::TurnResult>> {
        Box::pin(async move {
            super::answer::execute(&self.executions()?, caller, plan, connection, &self.root_id)
                .await
        })
    }
    fn resolve_model(
        &self,
        target: maka_protocol::session::SessionModelTarget,
        thinking: Option<maka_protocol::session::ThinkingLevel>,
    ) -> BoxFuture<'_, Result<crate::session::SessionModel>> {
        Box::pin(async move {
            crate::session::model::resolve(&self.executions()?.configuration, &target, thinking)
                .await
        })
    }
    fn configure_coordinator(
        &self,
        caller: Context,
        model: crate::plugins::workhub::coordinator::model::Prepared,
    ) -> BoxFuture<'_, Result<maka_event_log::sessions::SessionMutation<SessionConfiguration>>>
    {
        Box::pin(async move {
            let expected = model.expected_revision;
            let result = super::coordinator::configure(&self.executions()?, caller, model).await?;
            if matches!(
                &result,
                maka_event_log::sessions::SessionMutation::Committed(record) if record.revision != expected
            ) {
                self.coordinator_changed().await?;
            }
            Ok(result)
        })
    }
    fn coordinator(
        &self,
        caller: Context,
    ) -> BoxFuture<'_, Result<Option<maka_event_log::sessions::SessionRecord<SessionConfiguration>>>>
    {
        Box::pin(async move {
            let _call = caller
                .admit()
                .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
            self.executions()?.workhub_coordinator().await
        })
    }
    fn resolve_coordinator(
        &self,
        caller: Context,
        resolution: crate::plugins::workhub::coordinator::Resolution,
    ) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if super::coordinator::resolve(&self.executions()?, caller, resolution).await? {
                self.coordinator_changed().await?;
            }
            Ok(())
        })
    }
    fn selection(
        &self,
        caller: Context,
        input: maka_protocol::workhub::SelectionInput,
    ) -> BoxFuture<'_, Result<crate::plugins::workhub::selection::Source>> {
        Box::pin(async move { super::selection::inspect(&self.executions()?, caller, input).await })
    }
    fn offer_selection(
        &self,
        caller: Context,
        input: maka_protocol::workhub::SelectionInput,
        invocation: maka_runtime::event::Invocation,
        request: maka_runtime::interaction::InteractionRequest,
    ) -> BoxFuture<'_, Result<maka_runtime::interaction::InteractionRecord>> {
        Box::pin(async move {
            super::selection::offer(&self.executions()?, caller, input, invocation, request).await
        })
    }
    fn wait_selection(
        &self,
        request_id: String,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'_, Result<maka_runtime::interaction::InteractionOutcome>> {
        Box::pin(async move {
            self.executions()?
                .interactions
                .wait_for_outcome(&request_id, &cancellation)
                .await
        })
    }
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
        Box::pin(async move { super::correction::settle(&self.executions()?, identity).await })
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
        Box::pin(async move { super::stop::execute(&self.executions()?, caller, request).await })
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
