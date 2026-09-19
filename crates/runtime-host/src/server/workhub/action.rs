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

use super::Host;
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    workhub::{ActInput, ActResult},
};
use std::sync::Arc;
use uuid::Uuid;

pub(in crate::server) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::NotFound,
    Code::SessionArchived,
    Code::SessionBusy,
    Code::OperationConflict,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
    Code::CandidateSetStale,
];

pub(super) async fn act(
    host: &Arc<Host>,
    input: ActInput,
    connection: Uuid,
) -> Result<ActResult, OperationError> {
    match &input.proposal {
        maka_protocol::workhub::Proposal::Linked(proposal) => match proposal {
            maka_protocol::workhub::LinkedProposal::Correct { .. } => {
                super::control(host)?.value.correct(input).await
            }
            maka_protocol::workhub::LinkedProposal::Resume { .. } => {
                super::control(host)?.value.resume(input, connection).await
            }
            maka_protocol::workhub::LinkedProposal::Stop { .. } => {
                super::control(host)?.value.stop(input).await
            }
        },
        maka_protocol::workhub::Proposal::Route(_) => {
            super::control(host)?.value.delegate(input, None).await
        }
    }
}
