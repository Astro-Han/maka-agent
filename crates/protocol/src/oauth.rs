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

use crate::{Operation, OperationErrorCode, ProtocolError, Result, codec};
use maka_runtime::configuration::validation;
pub use maka_runtime::oauth::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

pub const PRESENTATION_SERVICE_ID: &str = "oauth_presentation";
pub const PRESENTATION_SERVICE_VERSION: &str = "1";

pub fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::OauthEnrollmentQuery
            | Operation::OauthLoginStart
            | Operation::OauthLoginQuery
            | Operation::OauthLoginCancel
    )
}
fn parsed<T: DeserializeOwned>(value: &Value) -> Result<T> {
    codec::record(value, "OAuth payload")?;
    serde_json::from_value(value.clone())
        .map_err(|_| ProtocolError::invalid("Invalid OAuth payload"))
}
fn id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ProtocolError::invalid("Invalid OAuth identity"));
    }
    Ok(())
}
fn encoded(value: impl Serialize) -> Result<Value> {
    serde_json::to_value(value).map_err(|_| ProtocolError::invalid("Invalid OAuth payload"))
}
pub fn decode_start(value: &Value) -> Result<LoginStart> {
    let input: LoginStart = parsed(value)?;
    id(&input.attempt_id)?;
    for key in ["slug", "name"] {
        if value["target"].get(key).is_some_and(Value::is_null) {
            return Err(ProtocolError::invalid("Invalid OAuth identity"));
        }
    }
    input
        .target
        .validate_create_identity()
        .map_err(ProtocolError::invalid)?;
    if let Target::Existing { connection_id } = &input.target {
        id(connection_id)?;
    }
    Ok(input)
}
pub fn decode_attempt(value: &Value) -> Result<Attempt> {
    let input: Attempt = parsed(value)?;
    id(&input.attempt_id)?;
    Ok(input)
}
pub fn decode_enrollment(value: &Value) -> Result<EnrollmentQuery> {
    parsed(value)
}
pub fn decode_enrollment_result(value: &Value) -> Result<EnrollmentProjection> {
    parsed(value)
}
pub fn decode_login(value: &Value) -> Result<LoginProjection> {
    let row = codec::record(value, "OAuth login projection")?;
    codec::exact(
        row,
        if value["phase"] == "failed" {
            &["attemptId", "connection", "phase", "failure"]
        } else {
            &["attemptId", "connection", "phase"]
        },
    )?;
    let output: LoginProjection = parsed(value)?;
    codec::record(&value["connection"], "OAuth connection identity")?;
    id(&output.attempt_id)?;
    id(&output.connection.connection_id)?;
    validation::slug(&output.connection.slug).map_err(ProtocolError::invalid)?;
    Ok(output)
}
pub fn assert_start(input: &LoginStart, output: &LoginProjection) -> Result<()> {
    if input.attempt_id != output.attempt_id || !input.target.matches(&output.connection) {
        return Err(ProtocolError::invalid(
            "OAuth login changed attempt or target identity",
        ));
    }
    Ok(())
}
pub fn assert_attempt(input: &Attempt, output: &LoginProjection) -> Result<()> {
    if input.attempt_id != output.attempt_id {
        return Err(ProtocolError::invalid(
            "OAuth login changed attempt identity",
        ));
    }
    Ok(())
}
pub fn decode_presentation(method: &str, value: &Value) -> Result<PresentationRequest> {
    if method != "open_external" {
        return Err(ProtocolError::invalid("Invalid OAuth presentation method"));
    }
    let row = codec::record(value, "OAuth presentation input")?;
    codec::shaped(row, &["url"], &["stateHint"])?;
    Ok(PresentationRequest::OpenExternal {
        url: codec::string(&row["url"], "OAuth presentation URL", 8192)?,
        state_hint: row
            .get("stateHint")
            .map(|v| codec::string(v, "OAuth state hint", 1024))
            .transpose()?,
    })
}
pub fn decode_presentation_result(method: &str, value: &Value) -> Result<PresentationResult> {
    if method != "open_external" {
        return Err(ProtocolError::invalid("Invalid OAuth presentation method"));
    }
    codec::exact(
        codec::record(value, "OAuth presentation result")?,
        &["kind"],
    )?;
    parsed(value)
}
pub fn decode_input(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::OauthLoginStart => encoded(decode_start(value)?),
        Operation::OauthLoginQuery | Operation::OauthLoginCancel => encoded(decode_attempt(value)?),
        Operation::OauthEnrollmentQuery => encoded(decode_enrollment(value)?),
        _ => Err(ProtocolError::invalid("Not an OAuth operation")),
    }
}
pub fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::OauthLoginStart | Operation::OauthLoginQuery | Operation::OauthLoginCancel => {
            encoded(decode_login(value)?)
        }
        Operation::OauthEnrollmentQuery => encoded(decode_enrollment_result(value)?),
        _ => Err(ProtocolError::invalid("Not an OAuth operation")),
    }
}
pub fn errors(operation: Operation) -> Option<&'static [OperationErrorCode]> {
    use OperationErrorCode::*;
    const COMMON: &[OperationErrorCode] = &[
        HostNotReady,
        HostDraining,
        OperationUnavailable,
        InvalidRequest,
        InternalFailure,
    ];
    const START: &[OperationErrorCode] = &[
        HostNotReady,
        HostDraining,
        OperationUnavailable,
        InvalidRequest,
        InternalFailure,
        OperationConflict,
        SlugTaken,
        CapabilityUnavailable,
        NotFound,
        PersistenceFailed,
    ];
    const ATTEMPT: &[OperationErrorCode] = &[
        HostNotReady,
        HostDraining,
        OperationUnavailable,
        InvalidRequest,
        InternalFailure,
        NotFound,
        PersistenceFailed,
    ];
    match operation {
        Operation::OauthEnrollmentQuery => Some(COMMON),
        Operation::OauthLoginStart => Some(START),
        Operation::OauthLoginQuery | Operation::OauthLoginCancel => Some(ATTEMPT),
        _ => None,
    }
}
