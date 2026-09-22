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
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Installation state, not a claim that every filesystem policy is supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    NotRequired,
    NotConfigured,
    SetupRequired,
    Ready,
    Removing,
    Busy,
}

pub fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::SandboxSetupQuery
            | Operation::SandboxSetupInstall
            | Operation::SandboxSetupRemove
    )
}

pub fn decode_input(value: &Value) -> Result<()> {
    codec::exact(codec::record(value, "sandbox setup input")?, &[])
}

pub fn decode_output(value: &Value) -> Result<Status> {
    serde_json::from_value(value.clone()).map_err(|error| ProtocolError::invalid(error.to_string()))
}

pub const ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::Unauthorized,
    OperationErrorCode::UserCancelled,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::OperationConflict,
    OperationErrorCode::OutcomeUnknown,
    OperationErrorCode::InternalFailure,
];
