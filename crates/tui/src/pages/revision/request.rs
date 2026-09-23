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

use maka_client::{Client, ClientError, RequestFailure};
use maka_protocol::{
    OperationErrorCode,
    session::{copy, sources},
    turn::{TurnBatchStartInput, TurnQueryInput, TurnStartResult},
};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Job {
    Load { session: String, turn: String },
    Copy(copy::Input),
    CopyQuery(copy::Input),
    Start(TurnBatchStartInput),
    TurnQuery(TurnQueryInput),
    Abandon(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub(super) root: String,
    pub(super) epoch: String,
    pub(super) sequence: u64,
    pub(super) job: Job,
}
impl Request {
    pub fn needs_checkpoint(&self) -> bool {
        matches!(self.job, Job::Copy(_) | Job::Start(_) | Job::Abandon(_))
    }
}
pub enum Output {
    Sources(sources::Output),
    Missing,
    Conflict,
    Started,
    Blocked(String),
    Abandoned,
    Retained,
}
pub async fn execute(client: &Client, request: &Request) -> Result<Output, RequestFailure> {
    let input = match &request.job {
        Job::Load { session, turn } => sources::Input {
            session_id: session.clone(),
            turn_id: turn.clone(),
        },
        Job::Copy(input) => {
            match client.copy_session(input.clone()).await {
                Ok(copy::Output::SourceRevisionConflict { .. }) => return Ok(Output::Conflict),
                Ok(copy::Output::Committed { .. }) => {}
                Err(RequestFailure::Rejected(ClientError::Rejected(error)))
                    if error.code != OperationErrorCode::CommitOutcomeUnknown =>
                {
                    return Ok(Output::Conflict);
                }
                Err(error) => return Err(error),
            };
            target_sources(input)
        }
        Job::CopyQuery(input) => {
            let output = client.query_session_copy(input.clone()).await?;
            let Some(receipt) = output.receipt else {
                return Ok(Output::Missing);
            };
            if receipt.state == copy::State::Abandoned {
                return Ok(Output::Abandoned);
            }
            target_sources(input)
        }
        Job::Start(input) => {
            return Ok(match client.start_turn_batch(input.clone()).await? {
                TurnStartResult::Started { .. } => Output::Started,
                TurnStartResult::Blocked { message, .. } => Output::Blocked(message),
            });
        }
        Job::TurnQuery(input) => {
            return match client.query_turn(input.clone()).await {
                Ok(_) => Ok(Output::Started),
                Err(RequestFailure::Rejected(ClientError::Rejected(error)))
                    if error.code == OperationErrorCode::NotFound =>
                {
                    Ok(Output::Missing)
                }
                Err(error) => Err(error),
            };
        }
        Job::Abandon(target) => {
            return Ok(
                match client
                    .abandon_session_revision(copy::AbandonInput {
                        target_session_id: target.clone(),
                    })
                    .await?
                {
                    copy::AbandonOutput::Abandoned { .. } => Output::Abandoned,
                    copy::AbandonOutput::Retained { .. } => Output::Retained,
                },
            );
        }
    };
    Ok(Output::Sources(client.session_turn_sources(input).await?))
}
fn target_sources(input: &copy::Input) -> sources::Input {
    let copy::Purpose::Revision { turn_id } = &input.purpose else {
        unreachable!()
    };
    sources::Input {
        session_id: input.target_session_id.clone(),
        turn_id: turn_id.clone(),
    }
}
