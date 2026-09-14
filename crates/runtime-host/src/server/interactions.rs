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

mod approval;
mod forms;
mod operations;
mod publication;
mod question;
mod waiting;
pub(super) use operations::{decode_input, decode_output, errors, execute, supports};

use crate::session::SessionConfiguration;
use maka_client_capability::broker::FormFuture;
use maka_event_log::{EventLog, StoreError};
use maka_protocol::{OperationError, OperationErrorCode as Code};
use maka_runtime::interaction::InteractionRecord;
use maka_runtime::{capability::FormInput, interaction::GrantTarget};
use maka_tools::{ApprovalFuture, ClientInteractions, ToolCallContext};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Live waiters read canonical outcomes; no second in-memory decision store.
#[derive(Clone)]
pub(crate) struct Interactions {
    log: Arc<EventLog>,
    admission: Arc<tokio::sync::Mutex<()>>,
    shutdown: CancellationToken,
    epoch: String,
}
impl ClientInteractions for Interactions {
    fn permission_mode(&self, context: ToolCallContext) -> maka_tools::PermissionFuture {
        let owner = self.clone();
        Box::pin(async move {
            owner
                .log
                .get_session::<SessionConfiguration>(&context.invocation.session_id)
                .await
                .map_err(
                    |error| maka_runtime::tool_call::ToolRejection::PreparationFailed {
                        message: error.to_string(),
                    },
                )?
                .filter(|record| !record.archived)
                .map(|record| record.configuration.permission_mode)
                .ok_or_else(
                    || maka_runtime::tool_call::ToolRejection::PreparationFailed {
                        message: "Session boundary is unavailable".into(),
                    },
                )
        })
    }

    fn approve(
        &self,
        target: GrantTarget,
        context: ToolCallContext,
        cancellation: CancellationToken,
        provider: CancellationToken,
    ) -> ApprovalFuture {
        self.approval(target, context, cancellation, provider)
    }

    fn form(
        &self,
        context: ToolCallContext,
        input: FormInput,
        cancellation: CancellationToken,
    ) -> FormFuture {
        let owner = self.clone();
        Box::pin(async move { owner.request_form(context, input, cancellation).await })
    }
}
impl Interactions {
    pub(crate) fn new(log: Arc<EventLog>, shutdown: CancellationToken, epoch: String) -> Self {
        Self {
            log,
            admission: Arc::new(tokio::sync::Mutex::new(())),
            shutdown,
            epoch,
        }
    }

    pub(crate) async fn lock_admission(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.admission.lock().await
    }

    /// Caller holds the shared admission gate, before cancelling the Run.
    pub(crate) async fn stop_run(
        &self,
        invocation: &maka_runtime::event::Invocation,
    ) -> Result<(), OperationError> {
        let now = super::configuration::now()
            .map_err(|error| failure(Code::InternalFailure, &error.to_string()))?;
        self.log
            .close_run_interactions(
                invocation,
                maka_runtime::interaction::ClosureReason::TurnStopped,
                now,
            )
            .await
            .map_err(|error| self.store_failure(error))?;
        Ok(())
    }

    async fn query_record(
        &self,
        session_id: &str,
        request_id: &str,
    ) -> Result<InteractionRecord, OperationError> {
        if self
            .log
            .get_session::<SessionConfiguration>(session_id)
            .await
            .map_err(|error| self.store_failure(error))?
            .is_none()
        {
            return Err(failure(Code::NotFound, "Interaction does not exist"));
        }
        self.log
            .interaction(request_id)
            .await
            .map_err(|error| self.store_failure(error))?
            .filter(|record| record.session_id == session_id)
            .ok_or_else(|| failure(Code::NotFound, "Interaction does not exist"))
    }

    fn store_failure(&self, error: StoreError) -> OperationError {
        // Losing canonical admission/decision authority invalidates this Host.
        self.shutdown.cancel();
        failure(Code::InternalFailure, &error.to_string())
    }
}
fn failure(code: Code, message: &str) -> OperationError {
    OperationError {
        code,
        message: message.chars().take(1024).collect(),
    }
}
