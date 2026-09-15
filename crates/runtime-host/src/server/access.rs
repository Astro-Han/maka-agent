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

use super::{Host, authority::Authority, configuration};
use maka_protocol::{Operation, OperationError, OperationErrorCode, ProtocolError};
use maka_runtime::access::{
    AccessCredentialFinalizeResult, AccessCredentialIssueResult, AccessCredentialRevokeResult,
};
use serde_json::Value;

use crate::access_delivery as delivery;
mod issuance;

#[derive(serde::Serialize)]
#[serde(untagged)]
pub(super) enum Output {
    Issue(AccessCredentialIssueResult),
    Revoke(AccessCredentialRevokeResult),
    Finalize(AccessCredentialFinalizeResult),
}

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::AccessCredentialIssue
            | Operation::AccessCredentialReplace
            | Operation::AccessCredentialPrepare
            | Operation::AccessCredentialFinalize
            | Operation::AccessCredentialRevoke
    )
}

pub(super) fn decode_input(operation: Operation, value: &Value) -> Result<Value, ProtocolError> {
    use maka_protocol::access::*;
    match operation {
        Operation::AccessCredentialIssue | Operation::AccessCredentialReplace => {
            decode_issue_input(value)?;
        }
        Operation::AccessCredentialPrepare => {
            decode_prepare_input(value)?;
        }
        Operation::AccessCredentialFinalize => {
            decode_finalize_input(value)?;
        }
        Operation::AccessCredentialRevoke => {
            decode_revoke_input(value)?;
        }
        _ => return Err(ProtocolError::invalid("Unknown access operation")),
    }
    Ok(value.clone())
}

pub(super) fn decode_output(operation: Operation, value: &Value) -> Result<Value, ProtocolError> {
    use maka_protocol::access::*;
    match operation {
        Operation::AccessCredentialIssue
        | Operation::AccessCredentialReplace
        | Operation::AccessCredentialPrepare => {
            decode_issue_result(value)?;
        }
        Operation::AccessCredentialFinalize => {
            decode_finalize_result(value)?;
        }
        Operation::AccessCredentialRevoke => {
            decode_revoke_result(value)?;
        }
        _ => return Err(ProtocolError::invalid("Unknown access operation")),
    }
    Ok(value.clone())
}

pub(super) async fn purge(
    host_root: &maka_event_log::root::RootOwner,
) -> Result<(), super::HostError> {
    host_root.validate_current()?;
    let control = host_root.control_directory().to_owned();
    tokio::task::spawn_blocking(move || delivery::purge(&control)).await??;
    Ok(())
}

pub(super) async fn execute(
    host: &Host,
    operation: Operation,
    value: &Value,
    authority: &Authority,
    client_instance_id: &str,
) -> Result<Output, OperationError> {
    match operation {
        Operation::AccessCredentialIssue
        | Operation::AccessCredentialReplace
        | Operation::AccessCredentialPrepare => issuance::execute(host, operation, value)
            .await
            .map(Output::Issue),
        Operation::AccessCredentialRevoke => {
            let input = maka_protocol::access::decode_revoke_input(value).map_err(invalid)?;
            let change = host
                .configuration
                .revoke_access_credential(input.credential_id.clone(), timestamp()?)
                .await
                .map_err(configuration::failure)?;
            publish(host, change.revoked);
            Ok(Output::Revoke(AccessCredentialRevokeResult {
                credential_id: input.credential_id,
                revoked: change.value,
            }))
        }
        Operation::AccessCredentialFinalize => {
            let credential = authority
                .credential()
                .ok_or_else(|| invalid("A remote access credential is required"))?;
            let change = host
                .configuration
                .finalize_access_credential(
                    credential.credential_id.clone(),
                    client_instance_id.into(),
                    credential.client_instance_id().map(str::to_owned),
                    configuration::now().map_err(configuration::failure)?,
                )
                .await
                .map_err(configuration::failure)?;
            publish(host, change.revoked);
            Ok(Output::Finalize(change.value))
        }
        _ => unreachable!("registered access operation"),
    }
}

/// Wake the listener-owned expiry deadline only after the control transaction commits.
fn publish(host: &Host, revoked: Vec<String>) {
    host.access_changed.notify_one();
    for id in revoked {
        let _ = host.access_revocations.send(id);
    }
}

pub(super) async fn expire(host: &Host) -> Result<(), super::HostError> {
    let revoked = host
        .configuration
        .expire_access_credentials(configuration::now()?)
        .await?;
    publish(host, revoked);
    Ok(())
}

fn timestamp() -> Result<String, OperationError> {
    time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .map_err(|_| internal("Cannot format access timestamp"))
}

fn invalid(error: impl std::fmt::Display) -> OperationError {
    OperationError {
        code: OperationErrorCode::InvalidRequest,
        message: error.to_string(),
    }
}
fn internal(message: &str) -> OperationError {
    OperationError {
        code: OperationErrorCode::InternalFailure,
        message: message.into(),
    }
}
