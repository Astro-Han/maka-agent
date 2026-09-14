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
//! Typed public configuration ingress; no JavaScript validation dependency.
use crate::{Operation, ProtocolError, Result};
use maka_runtime::configuration::validation as v;
pub use maka_runtime::configuration::*;
use serde::de::DeserializeOwned;
use serde_json::Value;
fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone())
        .map_err(|_| ProtocolError::invalid("Invalid configuration payload"))
}
fn validated<T>(result: std::result::Result<T, String>) -> Result<T> {
    result.map_err(ProtocolError::invalid)
}
fn required(value: &Value, fields: &[&str]) -> Result<()> {
    if !fields.iter().all(|k| value.get(k).is_some()) {
        return Err(ProtocolError::invalid("Missing configuration field"));
    }
    Ok(())
}
fn nonnull_optional(value: &Value, fields: &[&str]) -> Result<()> {
    if fields
        .iter()
        .any(|k| value.get(k).is_some_and(Value::is_null))
    {
        return Err(ProtocolError::invalid("Invalid null configuration field"));
    }
    Ok(())
}
pub fn decode_create_connection_input(value: &Value) -> Result<CreateCatalogConnectionInput> {
    nonnull_optional(
        &value["connection"],
        &["baseUrl", "modelOverrides", "requestBodyOverlay"],
    )?;
    validated(v::normalize_create(decode(value)?))
}
pub fn decode_update_connection_input(value: &Value) -> Result<UpdateCatalogConnectionInput> {
    nonnull_optional(&value["changes"], &["baseUrl"])?;
    let mut input: UpdateCatalogConnectionInput = decode(value)?;
    validated(v::basis(&input.expected))?;
    validated(v::normalize_update(&mut input.changes, None))?;
    Ok(input)
}
pub fn decode_remove_connection_input(value: &Value) -> Result<RemoveCatalogConnectionInput> {
    let input: RemoveCatalogConnectionInput = decode(value)?;
    validated(v::basis(&input.expected))?;
    Ok(input)
}
pub fn decode_set_default_target_input(value: &Value) -> Result<SetDefaultConnectionTargetInput> {
    required(value, &["target"])?;
    let input: SetDefaultConnectionTargetInput = decode(value)?;
    validated(v::revision(input.expected_catalog_revision, false))?;
    if let Some(target) = &input.target {
        validated(v::target(target))?;
    }
    Ok(input)
}
pub fn decode_credential_query_input(value: &Value) -> Result<CredentialVaultQueryInput> {
    let input: CredentialVaultQueryInput = decode(value)?;
    validated(v::locator(&input.locator))?;
    Ok(input)
}
pub fn decode_set_credential_input(value: &Value) -> Result<SetCredentialInput> {
    required(value, &["expected"])?;
    nonnull_optional(value, &["expectedConnection"])?;
    let mut input: SetCredentialInput = decode(value)?;
    validated(v::normalize_set_credential(&mut input))?;
    Ok(input)
}
pub fn decode_delete_credential_input(value: &Value) -> Result<DeleteCredentialInput> {
    let input: DeleteCredentialInput = decode(value)?;
    validated(v::credential_basis(&input.expected))?;
    Ok(input)
}
pub fn decode_catalog_query_input(value: &Value) -> Result<ConnectionCatalogQueryInput> {
    let input: ConnectionCatalogQueryInput = decode(value)?;
    if let ConnectionCatalogQueryInput::Continue { revision, cursor } = &input {
        validated(v::revision(*revision, false))?;
        let (connection_index, item) = match cursor {
            ConnectionCatalogCursor::Connection { connection_index } => (*connection_index, None),
            ConnectionCatalogCursor::EnabledModelId {
                connection_index,
                item_index,
            } => (*connection_index, Some((*item_index, 512))),
            ConnectionCatalogCursor::Model {
                connection_index,
                item_index,
            } => (*connection_index, Some((*item_index, 2048))),
            ConnectionCatalogCursor::CatalogEntry {
                connection_index,
                item_index,
            } => (
                *connection_index,
                Some((
                    *item_index,
                    crate::configuration_pages::MAX_CATALOG_ENTRIES as usize,
                )),
            ),
        };
        if connection_index >= 1024 || item.is_some_and(|(n, max)| n >= max) {
            return Err(ProtocolError::invalid("Invalid catalog cursor"));
        }
    }
    Ok(input)
}
pub fn decode_catalog_mutation_result(
    operation: Operation,
    value: &Value,
) -> Result<CatalogMutationResult> {
    let result: CatalogMutationResult = decode(value)?;
    let allowed = match &result {
        CatalogMutationResult::Committed {
            catalog_revision,
            connection,
        } => {
            validated(v::revision(*catalog_revision, false))?;
            let row = matches!(
                operation,
                Operation::ConnectionCatalogCreate | Operation::ConnectionCatalogUpdate
            );
            if row {
                required(value, &["connection"])?;
                if connection.is_none() {
                    return Err(ProtocolError::invalid("Missing committed connection"));
                }
            } else if value.get("connection").is_some() {
                return Err(ProtocolError::invalid("Unexpected committed connection"));
            }
            if let Some(basis) = connection {
                validated(v::basis(basis))?;
            }
            true
        }
        CatalogMutationResult::RevisionConflict {
            expected_revision,
            actual_revision,
        } => {
            validated(v::revision(*expected_revision, false))?;
            validated(v::revision(*actual_revision, false))?;
            matches!(
                operation,
                Operation::ConnectionCatalogCreate | Operation::ConnectionCatalogSetDefaultTarget
            )
        }
        CatalogMutationResult::ConnectionExists { slug } => {
            validated(v::slug(slug))?;
            operation == Operation::ConnectionCatalogCreate
        }
        CatalogMutationResult::ConnectionStale { expected, actual } => {
            required(value, &["actual"])?;
            validated(v::basis(expected))?;
            if let Some(actual) = actual {
                validated(v::basis(actual))?;
            }
            matches!(
                operation,
                Operation::ConnectionCatalogUpdate | Operation::ConnectionCatalogRemove
            )
        }
        CatalogMutationResult::InvalidDefaultTarget { target } => {
            validated(v::target(target))?;
            operation == Operation::ConnectionCatalogSetDefaultTarget
        }
    };
    if !allowed
        || ![
            Operation::ConnectionCatalogCreate,
            Operation::ConnectionCatalogUpdate,
            Operation::ConnectionCatalogRemove,
            Operation::ConnectionCatalogSetDefaultTarget,
        ]
        .contains(&operation)
    {
        return Err(ProtocolError::invalid("Unexpected catalog result"));
    }
    Ok(result)
}
pub fn decode_credential_query_result(value: &Value) -> Result<CredentialVaultQueryResult> {
    let result: CredentialVaultQueryResult = decode(value)?;
    if let CredentialVaultQueryResult::Status { status } = &result {
        required(&value["status"], &["credentialId", "revision", "updatedAt"])?;
        validated(v::credential_status(status))?;
    }
    Ok(result)
}
pub fn decode_credential_mutation_result(
    operation: Operation,
    value: &Value,
) -> Result<CredentialMutationResult> {
    if ![
        Operation::CredentialVaultSet,
        Operation::CredentialVaultDelete,
    ]
    .contains(&operation)
    {
        return Err(ProtocolError::invalid("Unexpected credential operation"));
    }
    let result: CredentialMutationResult = decode(value)?;
    match &result {
        CredentialMutationResult::Committed {
            vault_revision,
            status,
        } => {
            validated(v::revision(*vault_revision, false))?;
            required(&value["status"], &["credentialId", "revision", "updatedAt"])?;
            validated(v::credential_status(status))?;
        }
        CredentialMutationResult::ConnectionNotFound => {}
        CredentialMutationResult::ConnectionStale { expected, actual } => {
            if operation == Operation::CredentialVaultDelete {
                return Err(ProtocolError::invalid("Unexpected connection conflict"));
            }
            required(value, &["actual"])?;
            validated(v::basis(expected))?;
            if let Some(a) = actual {
                validated(v::basis(a))?;
            }
        }
        CredentialMutationResult::CredentialStale { expected, actual } => {
            required(value, &["expected", "actual"])?;
            for b in [expected, actual].into_iter().flatten() {
                validated(v::credential_basis(b))?;
            }
        }
    }
    Ok(result)
}
