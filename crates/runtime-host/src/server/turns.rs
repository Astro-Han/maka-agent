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

use maka_protocol::turn;
use maka_protocol::{Operation, OperationErrorCode as Code, ProtocolError, Result};
use serde_json::Value;

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::TurnStart
            | Operation::TurnQuery
            | Operation::TurnStop
            | Operation::TurnResumeQuery
            | Operation::TurnResumeStart
    )
}
pub(super) fn decode_input(operation: Operation, value: &Value) -> Result<Value> {
    let normalized = match operation {
        Operation::TurnStart => serde_json::to_value(turn::decode_turn_start_input(value)?),
        Operation::TurnQuery => serde_json::to_value(turn::decode_turn_query_input(value)?),
        Operation::TurnStop => serde_json::to_value(turn::decode_turn_stop_input(value)?),
        Operation::TurnResumeQuery => {
            serde_json::to_value(turn::decode_turn_resume_query_input(value)?)
        }
        Operation::TurnResumeStart => {
            serde_json::to_value(turn::decode_turn_resume_start_input(value)?)
        }
        _ => return Err(ProtocolError::invalid("Unknown Turn operation")),
    };
    normalized.map_err(|error| ProtocolError::invalid(error.to_string()))
}
pub(super) fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::TurnStart => {
            turn::decode_turn_start_result(value)?;
        }
        Operation::TurnResumeQuery => {
            turn::decode_turn_resume_plan(value)?;
        }
        Operation::TurnResumeStart => {
            turn::decode_turn_resume_start_result(value)?;
        }
        _ => {
            turn::decode_turn_snapshot(value)?;
        }
    }
    Ok(value.clone())
}
pub(super) fn errors(operation: Operation) -> Option<&'static [Code]> {
    match operation {
        Operation::TurnStart | Operation::TurnResumeStart => Some(&[
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::NotFound,
            Code::SessionArchived,
            Code::SessionBusy,
            Code::OperationConflict,
            Code::InternalFailure,
        ]),
        Operation::TurnResumeQuery => Some(&[
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::NotFound,
            Code::SessionArchived,
            Code::InternalFailure,
        ]),
        Operation::TurnQuery => Some(&[
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::NotFound,
            Code::InternalFailure,
        ]),
        Operation::TurnStop => Some(&[
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::NotFound,
            Code::OperationConflict,
            Code::InternalFailure,
        ]),
        _ => None,
    }
}
