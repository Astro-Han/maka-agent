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
        Operation::ProjectCatalogQuery | Operation::ProjectCatalogMutate
    )
}

pub fn decode_input(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::ProjectCatalogQuery => {
            super::decode_query(value)?;
        }
        Operation::ProjectCatalogMutate => {
            super::decode_mutation(value)?;
        }
        _ => return Err(ProtocolError::invalid("unknown Project operation")),
    }
    Ok(value.clone())
}

pub fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::ProjectCatalogQuery => {
            super::decode_query_result(value)?;
        }
        Operation::ProjectCatalogMutate => {
            super::decode_mutation_result(value)?;
        }
        _ => return Err(ProtocolError::invalid("unknown Project operation")),
    }
    Ok(value.clone())
}

pub const QUERY_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::InternalFailure,
];
pub const MUTATION_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::NotFound,
    Code::OperationConflict,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
];
