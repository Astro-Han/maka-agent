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

mod ingest;
mod preview;
mod query;
mod staging;
use super::{Host, HostError};
use maka_event_log::StoreError;
use maka_protocol::{
    Operation, OperationError, OperationErrorCode as Code, OperationMode, Outcome, artifact::*,
};
use serde_json::Value;
pub(super) use staging::Uploads;

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ArtifactIngest | Operation::ArtifactQuery | Operation::ArtifactDelete
    )
}

pub(super) fn errors(operation: Operation) -> Option<&'static [Code]> {
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

pub(super) fn decode_input(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    let result = match operation {
        Operation::ArtifactIngest => serde_json::to_value(decode_ingest_input(value)?),
        Operation::ArtifactQuery => serde_json::to_value(decode_query_input(value)?),
        Operation::ArtifactDelete => serde_json::to_value(decode_delete_input(value)?),
        _ => unreachable!("Artifact operation"),
    };
    result.map_err(|error| maka_protocol::ProtocolError::invalid(error.to_string()))
}
pub(super) fn decode_output(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    let result = match operation {
        Operation::ArtifactIngest => serde_json::to_value(decode_ingest_result(value)?),
        Operation::ArtifactQuery => serde_json::to_value(decode_query_result(value)?),
        Operation::ArtifactDelete => serde_json::to_value(decode_delete_result(value)?),
        _ => unreachable!("Artifact operation"),
    };
    result.map_err(|error| maka_protocol::ProtocolError::invalid(error.to_string()))
}

pub(super) async fn execute(
    host: &Host,
    connection: uuid::Uuid,
    operation: Operation,
    input: &Value,
) -> Result<Outcome, HostError> {
    let result = match operation {
        Operation::ArtifactIngest => ingest::execute(host, connection, decode_ingest_input(input)?)
            .await
            .and_then(encode),
        Operation::ArtifactQuery => query::execute(host, decode_query_input(input)?)
            .await
            .and_then(encode),
        Operation::ArtifactDelete => delete(host, decode_delete_input(input)?)
            .await
            .and_then(encode),
        _ => unreachable!("Artifact operation"),
    };
    if operation.mode() != OperationMode::Query
        && result
            .as_ref()
            .is_err_and(|error| error.code == Code::PersistenceFailed)
    {
        host.draining.cancel();
    }
    Ok(match result {
        Ok(value) => Outcome::success(decode_output(operation, &value)?),
        Err(error) => Outcome::failure(error),
    })
}

async fn delete(
    host: &Host,
    input: ArtifactDeleteInput,
) -> Result<ArtifactDeleteResult, OperationError> {
    use maka_event_log::artifacts::ArtifactDeletion;
    match host
        .log
        .delete_user_artifact(&input.session_id, &input.artifact_id)
        .await
        .map_err(store_error)?
    {
        ArtifactDeletion::Deleted => Ok(ArtifactDeleteResult::Deleted {}),
        ArtifactDeletion::NotFound => Err(error(Code::NotFound, "Artifact was not found")),
        ArtifactDeletion::Protected => Err(error(
            Code::OperationConflict,
            "Runtime-owned evidence cannot be deleted independently of its workflow",
        )),
    }
}
fn encode(value: impl serde::Serialize) -> Result<Value, OperationError> {
    serde_json::to_value(value).map_err(|_| error(Code::InternalFailure, "Invalid Artifact result"))
}
fn error(code: Code, message: &str) -> OperationError {
    OperationError {
        code,
        message: message.into(),
    }
}
fn store_error(cause: StoreError) -> OperationError {
    match cause {
        StoreError::SessionNotFound => error(Code::NotFound, "Session was not found"),
        StoreError::ArtifactConflict => error(
            Code::OperationConflict,
            "Artifact identity or content conflicts",
        ),
        StoreError::ArtifactOffset => {
            error(Code::InvalidRequest, "Artifact chunk offset is invalid")
        }
        _ => error(Code::PersistenceFailed, "Artifact storage is unavailable"),
    }
}
