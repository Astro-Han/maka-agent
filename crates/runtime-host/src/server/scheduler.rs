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
use crate::plugins::scheduler::{ID, Service};
use maka_plugins::{composition::Scope, storage::StoreError};
use maka_protocol::{
    Operation, OperationError, OperationErrorCode as Code, Outcome, ProtocolError,
};
use maka_scheduler::{
    Error,
    authorization::Origin,
    command::{Mutation, MutationResult, Query, QueryResult},
};
use serde_json::Value;

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ScheduledTaskQuery | Operation::ScheduledTaskMutate
    )
}

pub(super) fn decode_input(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    match operation {
        Operation::ScheduledTaskQuery => {
            let query: Query = serde_json::from_value(value.clone()).map_err(invalid)?;
            query.validate().map_err(invalid)?;
        }
        Operation::ScheduledTaskMutate => {
            serde_json::from_value::<Mutation>(value.clone()).map_err(invalid)?;
        }
        _ => return Err(invalid("unknown scheduled-task operation")),
    }
    Ok(value.clone())
}
pub(super) fn decode_output(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    if serde_json::to_vec(value).map_err(invalid)?.len() > 88 * 1024 {
        return Err(invalid("scheduled-task result exceeds byte limit"));
    }
    match operation {
        Operation::ScheduledTaskQuery => {
            serde_json::from_value::<QueryResult>(value.clone()).map_err(invalid)?;
        }
        Operation::ScheduledTaskMutate => {
            serde_json::from_value::<MutationResult>(value.clone()).map_err(invalid)?;
        }
        _ => return Err(invalid("unknown scheduled-task operation")),
    }
    Ok(value.clone())
}

pub(super) async fn execute(
    host: &Host,
    operation: Operation,
    input: &Value,
) -> Result<Outcome, HostError> {
    let snapshot = host
        .executions
        .plugin_catalog
        .snapshot::<Service>(&Scope::Profile);
    let Some(service) = snapshot.entries.get(ID) else {
        return Ok(Outcome::failure(OperationError {
            code: Code::OperationUnavailable,
            message: "Scheduler plugin is not active".into(),
        }));
    };
    let result = match operation {
        Operation::ScheduledTaskQuery => service
            .value
            .query(serde_json::from_value(input.clone()).map_err(invalid)?)
            .map(|result| serde_json::to_value(result).expect("typed scheduler query")),
        Operation::ScheduledTaskMutate => service
            .value
            .mutate(
                serde_json::from_value(input.clone()).map_err(invalid)?,
                Origin::User,
            )
            .await
            .map(|result| serde_json::to_value(result).expect("typed scheduler mutation")),
        _ => return Err(invalid("unknown scheduled-task operation").into()),
    };
    Ok(match result {
        Ok(result) => Outcome::success(result),
        Err(error) => Outcome::failure(OperationError {
            code: match &error {
                Error::NotFound => Code::NotFound,
                Error::Invalid(_) | Error::Time(_) => Code::InvalidRequest,
                Error::Busy | Error::Storage(StoreError::Conflict { .. }) => {
                    Code::OperationConflict
                }
                Error::Closed | Error::Unavailable(_) | Error::Storage(StoreError::Retired) => {
                    Code::OperationUnavailable
                }
                Error::OutcomeUnknown | Error::Storage(_) => Code::PersistenceFailed,
            },
            message: error.to_string(),
        }),
    })
}
fn invalid(error: impl ToString) -> ProtocolError {
    ProtocolError::invalid(error.to_string())
}

pub(super) const QUERY_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::InternalFailure,
];
pub(super) const MUTATION_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::InternalFailure,
    Code::NotFound,
    Code::OperationConflict,
];
