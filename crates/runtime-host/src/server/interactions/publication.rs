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

use super::{Interactions, failure};
use crate::{server::subscriptions, session::SessionConfiguration};
use maka_event_log::{observation::SessionProjection, turns::InvocationState};
use maka_protocol::{OperationError, OperationErrorCode as Code};
use maka_runtime::{
    event::Invocation,
    interaction::{InteractionRecord, InteractionRequest, MAX_SAFE_INTEGER},
};
use maka_tools::ToolCallContext;
use tokio_util::sync::CancellationToken;

impl Interactions {
    pub(super) async fn admit_request(
        &self,
        context: ToolCallContext,
        request: InteractionRequest,
        cancellation: &CancellationToken,
    ) -> Result<InteractionRecord, OperationError> {
        let _gate = self.admission.lock().await;
        self.admit_stable_request(
            context.invocation,
            uuid::Uuid::new_v4().to_string(),
            request,
            cancellation,
        )
        .await
    }

    /// Caller holds the shared admission gate through publication. A retried
    /// control request reuses the original offer and outcome.
    pub(in crate::server) async fn admit_stable_request(
        &self,
        invocation: Invocation,
        request_id: String,
        request: InteractionRequest,
        cancellation: &CancellationToken,
    ) -> Result<InteractionRecord, OperationError> {
        if let Some(record) = self
            .log
            .interaction(&request_id)
            .await
            .map_err(|error| self.store_failure(error))?
        {
            if record.session_id != invocation.session_id
                || record.turn_id != invocation.turn_id
                || record.run_id != invocation.run_id
                || record.request != request
            {
                return Err(failure(
                    Code::OperationConflict,
                    "Interaction identity belongs to another request",
                ));
            }
            return Ok(record);
        }
        if self.shutdown.is_cancelled() || cancellation.is_cancelled() {
            return Err(failure(
                Code::OperationConflict,
                "Interaction was cancelled",
            ));
        }
        request
            .validate()
            .map_err(|message| failure(Code::InvalidRequest, message))?;
        let observation = self.active_projection(&invocation).await?;
        let record = InteractionRecord {
            session_id: invocation.session_id,
            turn_id: invocation.turn_id,
            run_id: invocation.run_id,
            request_id,
            created_at: self.timestamp()?,
            request,
            outcome: None,
        };
        self.publish(observation, record).await
    }

    pub(super) fn timestamp(&self) -> Result<u64, OperationError> {
        crate::server::configuration::now().map_err(|error| {
            self.shutdown.cancel();
            failure(Code::InternalFailure, &error.to_string())
        })
    }

    /// Caller holds admission through validation and publication.
    pub(super) async fn active_projection(
        &self,
        invocation: &Invocation,
    ) -> Result<SessionProjection<SessionConfiguration>, OperationError> {
        let observation = self
            .log
            .session_projection::<SessionConfiguration>(&invocation.session_id)
            .await
            .map_err(|error| self.store_failure(error))?
            .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
        if !observation.root_turn.as_ref().is_some_and(|turn| {
            turn.invocation == *invocation && !matches!(turn.state, InvocationState::Ended { .. })
        }) {
            return Err(failure(
                Code::OperationConflict,
                "Interaction Run is no longer active",
            ));
        }
        Ok(observation)
    }

    pub(super) async fn publish(
        &self,
        mut observation: SessionProjection<SessionConfiguration>,
        record: InteractionRecord,
    ) -> Result<InteractionRecord, OperationError> {
        observation.pending_interactions.push(record.clone());
        if let Some(turn) = &mut observation.root_turn {
            turn.state = InvocationState::WaitingForUser;
        }
        // Reserve maximum revision encoding before a canonical request becomes visible.
        subscriptions::delivery::project(&self.epoch, MAX_SAFE_INTEGER, observation)
            .and_then(|snapshot| snapshot.validate().map_err(Into::into))
            .map_err(|_| {
                failure(
                    Code::OperationConflict,
                    "Interaction exceeds Session observation capacity",
                )
            })?;
        let committed = self
            .log
            .establish_interaction(&record)
            .await
            .map_err(|error| self.store_failure(error))?;
        if !committed.matches || committed.record.outcome.is_some() {
            self.shutdown.cancel();
            return Err(failure(
                Code::InternalFailure,
                "Interaction request identity conflicts with canonical state",
            ));
        }
        self.publish_catalog(&committed.record.session_id).await?;
        Ok(committed.record)
    }
}
