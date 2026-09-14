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

use super::{Result, failure, invalid, item, stored};
use crate::session::{SessionConfiguration, apply_metadata_patch};
use maka_event_log::{EventLog, StoreError, sessions::SessionMutation};
use maka_protocol::OperationErrorCode as Code;
use maka_protocol::session::*;
use serde_json::Value;

pub(super) async fn metadata(log: &EventLog, value: &Value) -> Result<SessionUpdateResult> {
    let input = decode_session_metadata_update_input(value).map_err(invalid)?;
    if input.session_id == "maka_workhub_coordination" {
        return Err(failure(
            Code::OperationUnavailable,
            "WorkHub metadata requires WorkHub authority",
        ));
    }
    let patch = input.patch.clone();
    let mutation = log
        .update_session_metadata(
            &input.session_id,
            input.expected_revision,
            move |config: &mut SessionConfiguration| {
                apply_metadata_patch(config, patch)
                    .map_err(|error| StoreError::InvalidTransition(error.message))
            },
        )
        .await
        .map_err(|error| match error {
            StoreError::InvalidTransition(message) => failure(Code::InvalidRequest, &message),
            error => stored(error),
        })?;
    let output = result(mutation);
    assert_metadata_update_output_for_input(&input, &output).map_err(invalid)?;
    Ok(output)
}

pub(in crate::server) fn result(
    mutation: SessionMutation<SessionConfiguration>,
) -> SessionUpdateResult {
    match mutation {
        SessionMutation::Committed(record) => SessionUpdateResult::Committed {
            session: item(record),
        },
        SessionMutation::RevisionConflict { expected, actual } => {
            SessionUpdateResult::RevisionConflict {
                expected_revision: expected,
                actual_revision: actual,
            }
        }
    }
}

pub(super) async fn read_marker(log: &EventLog, value: &Value) -> Result<SessionCatalogItem> {
    let input = decode_session_read_marker_set_input(value).map_err(invalid)?;
    if input.session_id == "maka_workhub_coordination" {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub read state requires WorkHub authority",
        ));
    }
    let record = log
        .set_session_read_marker(&input.session_id, &input.read_through_message_id)
        .await
        .map_err(stored)?;
    let output = item(record);
    assert_read_marker_output_for_input(&input, &output).map_err(invalid)?;
    Ok(output)
}

pub(crate) const METADATA_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::NotFound,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
];

pub(crate) const READ_MARKER_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::NotFound,
    Code::InvalidRequest,
    Code::OperationConflict,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
];
