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
use crate::{Operation, OperationErrorCode};

pub fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::SessionCreate
            | Operation::SessionBranchCreate
            | Operation::SessionRevisionCreate
            | Operation::SessionRevisionAbandon
            | Operation::SessionCopyQuery
            | Operation::SessionSourcesQuery
            | Operation::SessionCatalogQuery
            | Operation::SessionLifecycleSet
            | Operation::SessionMetadataUpdate
            | Operation::SessionReadMarkerSet
            | Operation::SessionConfigurationUpdate
            | Operation::SessionWorkspaceRelocate
            | Operation::SessionRemove
            | Operation::SessionRemovePreview
            | Operation::SessionRemoveQuery
    )
}

pub fn decode_input(operation: Operation, value: &Value) -> crate::Result<Value> {
    match operation {
        Operation::SessionRemove => {
            decode_session_remove_input(value)?;
        }
        Operation::SessionRemovePreview => {
            decode_session_remove_preview_input(value)?;
        }
        Operation::SessionRemoveQuery => {
            decode_session_remove_query_input(value)?;
        }
        Operation::SessionSourcesQuery => {
            crate::session::sources::decode_input(value)?;
        }
        Operation::SessionBranchCreate | Operation::SessionRevisionCreate => {
            crate::session::copy::decode_input(operation, value)?;
        }
        Operation::SessionRevisionAbandon => {
            crate::session::copy::decode_abandon_input(value)?;
        }
        Operation::SessionCopyQuery => {
            crate::session::copy::decode_query_input(value)?;
        }
        Operation::SessionCreate => {
            decode_session_create_input(value)?;
        }
        Operation::SessionCatalogQuery => {
            decode_session_catalog_query_input(value)?;
        }
        Operation::SessionLifecycleSet => {
            decode_session_lifecycle_set_input(value)?;
        }
        Operation::SessionMetadataUpdate => {
            decode_session_metadata_update_input(value)?;
        }
        Operation::SessionConfigurationUpdate => {
            decode_session_configuration_update_input(value)?;
        }
        Operation::SessionWorkspaceRelocate => {
            decode_session_workspace_relocate_input(value)?;
        }
        Operation::SessionReadMarkerSet => {
            decode_session_read_marker_set_input(value)?;
        }
        _ => return Err(ProtocolError::invalid("Unknown Session operation")),
    }
    Ok(value.clone())
}

pub fn decode_output(operation: Operation, value: &Value) -> crate::Result<Value> {
    if operation == Operation::SessionRemove {
        decode_session_remove_result(value)?;
    } else if operation == Operation::SessionRemovePreview {
        decode_session_remove_preview_result(value)?;
    } else if operation == Operation::SessionRemoveQuery {
        decode_session_remove_query_result(value)?;
    } else if matches!(
        operation,
        Operation::SessionBranchCreate | Operation::SessionRevisionCreate
    ) {
        crate::session::copy::decode_result(value)?;
    } else if operation == Operation::SessionSourcesQuery {
        crate::session::sources::decode_output(value)?;
    } else if operation == Operation::SessionCopyQuery {
        crate::session::copy::decode_query_result(value)?;
    } else if operation == Operation::SessionRevisionAbandon {
        crate::session::copy::decode_abandon_result(value)?;
    } else if operation == Operation::SessionCatalogQuery {
        decode_session_catalog_query_result(value)?;
    } else if matches!(
        operation,
        Operation::SessionMetadataUpdate
            | Operation::SessionConfigurationUpdate
            | Operation::SessionWorkspaceRelocate
    ) {
        decode_session_update_result(value)?;
    } else {
        decode_session_catalog_projection(value)?;
    }
    Ok(value.clone())
}

pub const QUERY_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::InternalFailure,
];
pub const CREATE_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::OperationConflict,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::CommitOutcomeUnknown,
    OperationErrorCode::InternalFailure,
];
pub const LIFECYCLE_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::NotFound,
    OperationErrorCode::SessionBusy,
    OperationErrorCode::OperationConflict,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::CommitOutcomeUnknown,
    OperationErrorCode::InternalFailure,
];

pub const METADATA_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::NotFound,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::CommitOutcomeUnknown,
    OperationErrorCode::InternalFailure,
];

pub const READ_MARKER_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::NotFound,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::OperationConflict,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::CommitOutcomeUnknown,
    OperationErrorCode::InternalFailure,
];

pub const CONFIGURATION_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::InternalFailure,
    OperationErrorCode::NotFound,
    OperationErrorCode::CommitOutcomeUnknown,
    OperationErrorCode::SessionBusy,
    OperationErrorCode::OperationConflict,
];

pub fn errors(operation: Operation) -> Option<&'static [OperationErrorCode]> {
    match operation {
        Operation::SessionBranchCreate
        | Operation::SessionRevisionCreate
        | Operation::SessionRevisionAbandon
        | Operation::SessionCopyQuery
        | Operation::SessionRemove
        | Operation::SessionRemovePreview
        | Operation::SessionRemoveQuery => Some(CONFIGURATION_ERRORS),
        Operation::SessionSourcesQuery => Some(crate::session::sources::ERRORS),
        Operation::SessionCreate => Some(CREATE_ERRORS),
        Operation::SessionCatalogQuery => Some(QUERY_ERRORS),
        Operation::SessionLifecycleSet => Some(LIFECYCLE_ERRORS),
        Operation::SessionMetadataUpdate => Some(METADATA_ERRORS),
        Operation::SessionReadMarkerSet => Some(READ_MARKER_ERRORS),
        Operation::SessionConfigurationUpdate | Operation::SessionWorkspaceRelocate => {
            Some(CONFIGURATION_ERRORS)
        }
        _ => None,
    }
}
