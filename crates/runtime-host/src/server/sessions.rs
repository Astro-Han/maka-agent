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

pub(super) mod configuration;
pub(super) mod create;
pub(super) use crate::session::model;
pub(super) mod mutation;
pub(super) mod workspace;

use crate::session::SessionConfiguration;
use maka_event_log::sessions::SessionRecord;
use maka_event_log::{EventLog, StoreError};
use maka_protocol::OperationErrorCode;
use maka_protocol::session::*;
use maka_protocol::{Operation, OperationError, ProtocolError};
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, OperationError>;

#[derive(serde::Serialize)]
#[serde(untagged)]
pub(super) enum Output {
    Query(SessionCatalogQueryResult),
    Item(SessionCatalogItem),
    Mutation(SessionUpdateResult),
}

impl Output {
    pub(super) fn refresh_catalog(&self) -> bool {
        matches!(
            self,
            Self::Item(_) | Self::Mutation(SessionUpdateResult::Committed { .. })
        )
    }
}

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::SessionCreate
            | Operation::SessionCatalogQuery
            | Operation::SessionLifecycleSet
            | Operation::SessionMetadataUpdate
            | Operation::SessionReadMarkerSet
            | Operation::SessionConfigurationUpdate
            | Operation::SessionWorkspaceRelocate
    )
}

pub(super) fn decode_input(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    match operation {
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

pub(super) fn decode_output(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    if operation == Operation::SessionCatalogQuery {
        decode_session_catalog_query_result(value)?;
    } else if matches!(
        operation,
        Operation::SessionMetadataUpdate
            | Operation::SessionConfigurationUpdate
            | Operation::SessionWorkspaceRelocate
    ) {
        decode_session_update_result(value)?;
    } else {
        decode_session_catalog_item(value)?;
    }
    Ok(value.clone())
}

pub(super) async fn execute(
    host: &super::Host,
    operation: Operation,
    value: &Value,
) -> Result<Output> {
    let log = host.log.as_ref();
    match operation {
        Operation::SessionCreate => {
            let input = decode_session_create_input(value).map_err(invalid)?;
            let item = create::create(host, input.clone()).await?;
            assert_create_output_for_input(&input, &item).map_err(invalid)?;
            Ok(Output::Item(item))
        }
        Operation::SessionCatalogQuery => query(
            log,
            decode_session_catalog_query_input(value).map_err(invalid)?,
        )
        .await
        .map(Output::Query),
        Operation::SessionLifecycleSet => {
            let input = decode_session_lifecycle_set_input(value).map_err(invalid)?;
            crate::session::require_unmanaged(
                log,
                &input.session_id,
                OperationErrorCode::OperationConflict,
            )
            .await?;
            let _admission = host.executions.lock_admission().await;
            if input.state == SessionLifecycleState::Archived
                && host
                    .executions
                    .has_session_work(&input.session_id)
                    .await
                    .map_err(stored)?
            {
                return Err(failure(
                    OperationErrorCode::SessionBusy,
                    "Session still owns live or pending work",
                ));
            }
            let record = log
                .set_session_archived(
                    &input.session_id,
                    input.state == SessionLifecycleState::Archived,
                    super::configuration::now().map_err(super::configuration::failure)?,
                )
                .await
                .map_err(stored)?;
            let item = item(record);
            assert_lifecycle_output_for_input(&input, &item).map_err(invalid)?;
            Ok(Output::Item(item))
        }
        Operation::SessionMetadataUpdate => {
            mutation::metadata(log, value).await.map(Output::Mutation)
        }
        Operation::SessionReadMarkerSet => {
            mutation::read_marker(log, value).await.map(Output::Item)
        }
        Operation::SessionConfigurationUpdate => configuration::update(host, value)
            .await
            .map(Output::Mutation),
        Operation::SessionWorkspaceRelocate => {
            workspace::relocate(host, value).await.map(Output::Mutation)
        }
        _ => Err(failure(
            OperationErrorCode::OperationUnavailable,
            "Session operation is not installed",
        )),
    }
}

async fn query(
    log: &EventLog,
    input: SessionCatalogQueryInput,
) -> Result<SessionCatalogQueryResult> {
    if let SessionCatalogQueryInput::Get { session_id } = &input {
        return Ok(SessionCatalogQueryResult::Session {
            session: log.get_session(session_id).await.map_err(stored)?.map(item),
        });
    }
    let (revision, cursor) = match &input {
        SessionCatalogQueryInput::ListContinue { revision, cursor } => {
            (Some(revision.as_str()), Some(cursor.as_str()))
        }
        _ => (None, None),
    };
    let mut page = match log
        .list_sessions::<SessionConfiguration>(revision, cursor, 32)
        .await
    {
        Ok(page) => page,
        Err(StoreError::RevisionConflict { expected, actual }) => {
            return Ok(SessionCatalogQueryResult::RevisionChanged {
                expected_revision: expected,
                actual_revision: actual,
            });
        }
        Err(error) => return Err(stored(error)),
    };
    let mut sessions = Vec::new();
    for record in page.sessions {
        let id = record.id.clone();
        sessions.push(item(record));
        let candidate =
            json!({"kind":"page","revision":page.revision,"sessions":sessions,"nextCursor":id});
        if candidate.to_string().len() > 48 * 1024 {
            sessions.pop();
            if sessions.is_empty() {
                return Err(failure(
                    OperationErrorCode::InternalFailure,
                    "Session item exceeds catalog page budget",
                ));
            }
            page.next_cursor = sessions.last().map(|item| item.id().to_owned());
            break;
        }
    }
    Ok(SessionCatalogQueryResult::Page {
        revision: page.revision,
        sessions,
        next_cursor: page.next_cursor,
    })
}

fn item(record: SessionRecord<SessionConfiguration>) -> SessionCatalogItem {
    SessionCatalogItem::Projection(Box::new(crate::session::catalog_projection(record)))
}

fn invalid(error: ProtocolError) -> OperationError {
    failure(OperationErrorCode::InvalidRequest, &error.message)
}

fn failure(code: OperationErrorCode, message: &str) -> OperationError {
    OperationError {
        code,
        message: message.chars().take(1024).collect(),
    }
}

pub(super) fn stored(error: StoreError) -> OperationError {
    let code = match &error {
        StoreError::SessionConflict => OperationErrorCode::OperationConflict,
        StoreError::SessionNotFound => OperationErrorCode::NotFound,
        StoreError::SessionBusy => OperationErrorCode::SessionBusy,
        StoreError::CommitUnknown(_) | StoreError::OperationUnknown => {
            OperationErrorCode::CommitOutcomeUnknown
        }
        _ => OperationErrorCode::PersistenceFailed,
    };
    failure(code, &error.to_string())
}

pub(super) const QUERY_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::InternalFailure,
];
pub(super) const CREATE_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::OperationConflict,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::CommitOutcomeUnknown,
    OperationErrorCode::InternalFailure,
];
pub(super) const LIFECYCLE_ERRORS: &[OperationErrorCode] = &[
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
