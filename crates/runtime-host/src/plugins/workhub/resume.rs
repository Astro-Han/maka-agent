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

use super::control::{Result, failure, fingerprint};
use maka_protocol::{
    OperationErrorCode as Code,
    workhub::{ActInput, ActResult, LinkedProposal, Proposal, ResumeOutcome},
};
use maka_runtime::workhub::ActionId;

pub(crate) struct Request {
    pub action_id: ActionId,
    pub turn_id: String,
    pub request_fingerprint: String,
    pub delegation_action_id: ActionId,
    pub target_session_id: String,
}

pub(crate) enum Receipt {
    Running { session_id: String },
    Started { session_id: String, turn_id: String },
}

impl super::Control {
    pub(crate) async fn resume(
        &self,
        input: ActInput,
        connection: uuid::Uuid,
    ) -> Result<ActResult> {
        let request_fingerprint = fingerprint(&input)?;
        let Proposal::Linked(LinkedProposal::Resume {
            resumes_action_id,
            expects,
        }) = input.proposal
        else {
            return Err(failure(
                Code::OperationConflict,
                "Not a WorkHub resume request",
            ));
        };
        let receipt = self
            .commands
            .resume(
                self.caller.clone(),
                Request {
                    action_id: input.action_id,
                    turn_id: input.turn_id,
                    request_fingerprint,
                    delegation_action_id: resumes_action_id,
                    target_session_id: expects.target_session_id,
                },
                connection,
                super::candidates::eligible,
            )
            .await?;
        Ok(match receipt {
            Receipt::Running { session_id } => ActResult::ResumeWork {
                outcome: ResumeOutcome::AlreadyRunning,
                target_session_id: session_id,
                target_turn_id: None,
            },
            Receipt::Started {
                session_id,
                turn_id,
            } => ActResult::ResumeWork {
                outcome: ResumeOutcome::ResumeStarted,
                target_session_id: session_id,
                target_turn_id: Some(turn_id),
            },
        })
    }
}
