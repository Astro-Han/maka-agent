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

use crate::{Operation, OperationErrorCode as Code, ProtocolError, Result};
use serde_json::Value;

pub fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::InteractionQuery | Operation::InteractionAnswer
    )
}
pub fn errors(operation: Operation) -> Option<&'static [Code]> {
    const QUERY: &[Code] = &[
        Code::HostNotReady,
        Code::HostDraining,
        Code::OperationUnavailable,
        Code::InvalidRequest,
        Code::NotFound,
        Code::InternalFailure,
    ];
    const ANSWER: &[Code] = &[
        Code::HostNotReady,
        Code::HostDraining,
        Code::OperationUnavailable,
        Code::InvalidRequest,
        Code::NotFound,
        Code::OperationConflict,
        Code::AlreadyResolved,
        Code::InternalFailure,
    ];
    match operation {
        Operation::InteractionQuery => Some(QUERY),
        Operation::InteractionAnswer => Some(ANSWER),
        _ => None,
    }
}
pub fn decode_input(operation: Operation, value: &Value) -> Result<Value> {
    let result = match operation {
        Operation::InteractionQuery => serde_json::to_value(super::decode_query_input(value)?),
        Operation::InteractionAnswer => serde_json::to_value(super::decode_answer_input(value)?),
        _ => return Err(ProtocolError::invalid("Unknown Interaction operation")),
    };
    result.map_err(|error| ProtocolError::invalid(error.to_string()))
}
pub fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    let snapshot = match operation {
        Operation::InteractionQuery => super::decode_snapshot(value)?,
        Operation::InteractionAnswer => super::decode_answered_snapshot(value)?,
        _ => return Err(ProtocolError::invalid("Unknown Interaction operation")),
    };
    serde_json::to_value(snapshot).map_err(|error| ProtocolError::invalid(error.to_string()))
}
