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

use super::{Host, HostError, sessions};
use crate::session::SessionConfiguration;
use maka_protocol::session::PermissionMode;
use maka_protocol::{
    OperationError, OperationErrorCode as Code, Outcome, execution_boundary as wire,
};
use serde_json::Value;

/// The same persisted policy configures tool scopes at admission. No memory/UI fallback.
/// Managed scopes do not provide OS process isolation: shell remains bypass-only.
pub(super) async fn execute(host: &Host, value: &Value) -> Result<Outcome, HostError> {
    let session_id = wire::decode_input(value)?;
    let record = match host
        .log
        .get_session::<SessionConfiguration>(&session_id)
        .await
    {
        Ok(Some(record)) => record,
        Ok(None) => {
            return Ok(Outcome::failure(OperationError {
                code: Code::NotFound,
                message: "Session does not exist".into(),
            }));
        }
        Err(error) => return Ok(Outcome::failure(sessions::stored(error))),
    };
    let config = record.configuration;
    let revision = config.boundary_revision;
    let boundary = match config.permission_mode {
        PermissionMode::Explore => wire::ExecutionBoundarySummary::Managed {
            access: wire::ManagedAccess::ReadOnly,
            revision,
        },
        PermissionMode::Ask => wire::ExecutionBoundarySummary::Managed {
            access: wire::ManagedAccess::Writable,
            revision,
        },
        PermissionMode::Bypass => wire::ExecutionBoundarySummary::Bypass { revision },
    };
    let output = serde_json::to_value(boundary)?;
    wire::decode_output(&output)?;
    Ok(Outcome::success(output))
}

pub(super) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::InternalFailure,
    Code::NotFound,
];
