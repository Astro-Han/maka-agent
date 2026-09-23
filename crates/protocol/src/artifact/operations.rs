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

use super::*;
use crate::{Operation, OperationErrorCode as Code, OperationMode};

pub fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ArtifactIngest | Operation::ArtifactQuery | Operation::ArtifactDelete
    )
}

pub fn errors(operation: Operation) -> Option<&'static [Code]> {
    const QUERY: &[Code] = &[
        Code::HostNotReady,
        Code::HostDraining,
        Code::OperationUnavailable,
        Code::InternalFailure,
        Code::InvalidRequest,
        Code::NotFound,
        Code::PersistenceFailed,
    ];
    const MUTATION: &[Code] = &[
        Code::HostNotReady,
        Code::HostDraining,
        Code::OperationUnavailable,
        Code::InternalFailure,
        Code::InvalidRequest,
        Code::NotFound,
        Code::PersistenceFailed,
        Code::OperationConflict,
    ];
    supports(operation).then_some(if operation.mode() == OperationMode::Query {
        QUERY
    } else {
        MUTATION
    })
}

pub fn decode_input(operation: Operation, value: &Value) -> crate::Result<Value> {
    let result = match operation {
        Operation::ArtifactIngest => serde_json::to_value(decode_ingest_input(value)?),
        Operation::ArtifactQuery => serde_json::to_value(decode_query_input(value)?),
        Operation::ArtifactDelete => serde_json::to_value(decode_delete_input(value)?),
        _ => return Err(ProtocolError::invalid("Unknown artifact operation")),
    };
    result.map_err(|error| crate::ProtocolError::invalid(error.to_string()))
}
pub fn decode_output(operation: Operation, value: &Value) -> crate::Result<Value> {
    let result = match operation {
        Operation::ArtifactIngest => serde_json::to_value(decode_ingest_result(value)?),
        Operation::ArtifactQuery => serde_json::to_value(decode_query_result(value)?),
        Operation::ArtifactDelete => serde_json::to_value(decode_delete_result(value)?),
        _ => return Err(ProtocolError::invalid("Unknown artifact operation")),
    };
    result.map_err(|error| crate::ProtocolError::invalid(error.to_string()))
}
