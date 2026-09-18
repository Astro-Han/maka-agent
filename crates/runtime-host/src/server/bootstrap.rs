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

use maka_protocol::OperationErrorCode;
use maka_protocol::codec::{exact, record};
use maka_protocol::{Operation, OperationRegistry, ProtocolError, Result, host};
use serde_json::{Value, json};

pub(super) struct Operations;

impl OperationRegistry for Operations {
    fn decode_input(&self, operation: Operation, value: &Value) -> Result<Value> {
        if operation == Operation::HostUpgradePrepare {
            maka_protocol::host::decode_retirement_input(value)?;
            return Ok(value.clone());
        }
        if !matches!(
            operation,
            Operation::HostStatus | Operation::HostDiagnosticsQuery | Operation::HostWake
        ) {
            return Err(ProtocolError::invalid("Unknown operation"));
        }
        exact(record(value, "host.status input")?, &[])?;
        Ok(json!({}))
    }
    fn decode_output(&self, operation: Operation, value: &Value) -> Result<Value> {
        if operation == Operation::HostUpgradePrepare {
            maka_protocol::host::decode_retirement_result(value)?;
            return Ok(value.clone());
        }
        match operation {
            Operation::HostWake => exact(record(value, "host.wake result")?, &[])?,
            Operation::HostStatus => host::decode_status(value)?,
            Operation::HostDiagnosticsQuery => host::decode_diagnostics(value)?,
            _ => return Err(ProtocolError::invalid("Unknown operation")),
        }
        Ok(value.clone())
    }
    fn error_codes(&self, operation: Operation) -> Option<&[OperationErrorCode]> {
        if operation == Operation::HostWake {
            return Some(&[
                OperationErrorCode::HostDraining,
                OperationErrorCode::OperationUnavailable,
                OperationErrorCode::InternalFailure,
            ]);
        }
        if operation == Operation::HostUpgradePrepare {
            return Some(&[
                OperationErrorCode::OperationConflict,
                OperationErrorCode::OperationUnavailable,
                OperationErrorCode::InternalFailure,
            ]);
        }
        matches!(
            operation,
            Operation::HostStatus | Operation::HostDiagnosticsQuery | Operation::HostWake
        )
        .then_some(
            &[
                OperationErrorCode::HostDraining,
                OperationErrorCode::InternalFailure,
            ][..],
        )
    }
}
