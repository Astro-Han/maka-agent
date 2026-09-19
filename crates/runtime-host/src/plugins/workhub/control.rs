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

use crate::session::SessionConfiguration;
use futures_util::future::BoxFuture;
use maka_event_log::workhub::{Candidate, stop::StopRecord};
use maka_plugins::fiber::Context;
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    workhub::{ActInput, ActResult, LinkedProposal, Proposal},
};
use maka_runtime::{artifact::content_digest, workhub::ActionId};
use std::sync::Arc;

pub(crate) type Result<T> = std::result::Result<T, OperationError>;
pub(crate) type CandidateFilter = fn(&str, &SessionConfiguration) -> bool;

/// A logical control request, never a request to stop whichever Run is current.
/// Host resolves the canonical delegation and atomically captures its exact owner.
pub(crate) struct Stop {
    pub action_id: ActionId,
    pub turn_id: String,
    pub request_fingerprint: String,
    pub target_session_id: String,
}

/// Commands own admission, durable receipts and settlement. No locks, arbitrary
/// log writes or Host handles are exposed to the business implementation.
pub(crate) trait Commands: Send + Sync {
    /// Apply the domain predicate inside one bounded read snapshot, before its limit.
    fn candidates(
        &self,
        eligible: CandidateFilter,
    ) -> BoxFuture<'_, Result<Vec<Candidate<SessionConfiguration>>>>;
    fn stop(&self, caller: Context, request: Stop) -> BoxFuture<'_, Result<StopRecord>>;
    fn target(
        &self,
        session: String,
        eligible: CandidateFilter,
    ) -> BoxFuture<'_, Result<Option<maka_event_log::sessions::SessionRecord<SessionConfiguration>>>>;
    fn prepare_session(
        &self,
        request: maka_protocol::session::SessionCreateInput,
    ) -> BoxFuture<'_, Result<crate::execution::Creation>>;
    fn resume(
        &self,
        caller: Context,
        request: super::resume::Request,
        connection: uuid::Uuid,
        eligible: CandidateFilter,
    ) -> BoxFuture<'_, Result<super::resume::Receipt>>;
}

pub(crate) struct Control {
    pub(super) commands: Arc<dyn Commands>,
    pub(super) caller: Context,
}
impl Control {
    pub async fn stop(&self, input: ActInput) -> Result<ActResult> {
        let fingerprint = fingerprint(&input)?;
        let Proposal::Linked(LinkedProposal::Stop { expects }) = input.proposal else {
            return Err(failure(
                Code::OperationConflict,
                "Not a WorkHub stop request",
            ));
        };
        let record = self
            .commands
            .stop(
                self.caller.clone(),
                Stop {
                    action_id: input.action_id,
                    turn_id: input.turn_id,
                    request_fingerprint: fingerprint,
                    target_session_id: expects.target_session_id,
                },
            )
            .await?;
        let result = record
            .resolution
            .ok_or_else(|| failure(Code::InternalFailure, "Stop resolution is missing"))?;
        Ok(ActResult::StopWork {
            outcome: result.outcome(),
            target_session_id: record.intent.request.target_session_id,
            target_turn_id: result.target_turn_id().map(str::to_owned),
        })
    }
}

pub(crate) fn fingerprint(input: &ActInput) -> Result<String> {
    serde_json::to_vec(input)
        .map(|bytes| content_digest(&bytes))
        .map_err(|error| failure(Code::InternalFailure, error.to_string()))
}
pub(super) fn failure(code: Code, message: impl Into<String>) -> OperationError {
    OperationError {
        code,
        message: message.into().chars().take(1024).collect(),
    }
}
