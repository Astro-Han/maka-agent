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

use super::{Host, HostError};
use maka_event_log::{StoreError, message_resolution::MessageResolution};
use maka_protocol::{Operation, OperationError, OperationErrorCode as Code, Outcome, message::*};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) mod capacity;
mod mutations;
pub(crate) mod projection;
mod update;

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::TurnMessageSubmit
            | Operation::TurnInterrupt
            | Operation::TurnMessageQuery
            | Operation::TurnMessageExecutionQuery
            | Operation::QueueRetract
            | Operation::QueueEntryRetract
            | Operation::QueueEntryPromote
            | Operation::QueueEntryUpdate
            | Operation::QueueEntriesReorder
    )
}
pub(super) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::NotFound,
    Code::SessionArchived,
    Code::SessionBusy,
    Code::OperationConflict,
    Code::OutcomeUnknown,
    Code::InternalFailure,
];
pub(super) async fn execute(
    host: &Host,
    connection_id: uuid::Uuid,
    operation: Operation,
    value: &Value,
) -> Result<Outcome, HostError> {
    let input = decode_input(operation, value)?;
    let result = match input {
        Input::Interrupt(input) => host
            .executions
            .interrupt(input, &host.epoch)
            .await
            .map(|result| Output::Interrupt(Box::new(result))),
        Input::Submit(input) => host
            .executions
            .submit(*input, connection_id, host.root_id(), &host.epoch)
            .await
            .map(Output::Submit),
        Input::Query(input) | Input::ExecutionQuery(input) => host
            .log
            .message_resolutions(&input.session_id, &input.message_ids)
            .await
            .map(|resolutions| query_result(operation, resolutions))
            .map_err(|error| storage_error(host, error)),
        input => mutations::execute(host, connection_id, operation, input).await,
    };
    Ok(match result {
        Ok(output) => {
            let value = serde_json::to_value(output)?;
            decode_output(operation, &value)?;
            Outcome::success(value)
        }
        Err(error) => Outcome::failure(error),
    })
}
fn query_result(operation: Operation, resolutions: Vec<MessageResolution>) -> Output {
    if operation == Operation::TurnMessageQuery {
        return Output::Query(QueryResult {
            cancelled_message_ids: resolutions
                .into_iter()
                .filter_map(|r| {
                    if let MessageResolution::Cancelled { message_id } = r {
                        Some(message_id)
                    } else {
                        None
                    }
                })
                .collect(),
        });
    }
    Output::Executions(ExecutionQueryResult {
        resolutions: resolutions
            .into_iter()
            .map(|r| match r {
                MessageResolution::Pending { message_id } => {
                    ExecutionResolution::Pending { message_id }
                }
                MessageResolution::Cancelled { message_id } => {
                    ExecutionResolution::Cancelled { message_id }
                }
                MessageResolution::Owned {
                    message_id,
                    invocation,
                } => ExecutionResolution::Owned {
                    message_id,
                    turn_id: invocation.turn_id,
                    run_id: invocation.run_id,
                },
            })
            .collect(),
    })
}
pub(super) fn storage_error(host: &Host, error: StoreError) -> OperationError {
    host.executions.message_storage_error(error)
}
fn failure(code: Code, message: &str) -> OperationError {
    OperationError {
        code,
        message: message.chars().take(1024).collect(),
    }
}
fn fingerprint(input: &Input) -> Result<String, OperationError> {
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(
            serde_json::to_vec(input)
                .map_err(|e| failure(Code::InternalFailure, &e.to_string()))?
        )
    ))
}
