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

use crate::{
    ProtocolError, Result,
    codec::{exact, record, shaped, string},
};
use maka_runtime::access::*;
use serde_json::Value;
use std::collections::HashSet;

pub const ACCESS_CREDENTIAL_MAX_GRANTS: usize = 256;
const ISSUE_FIELDS: &[&str] = &[
    "principalKind",
    "principalId",
    "operationGrants",
    "canPublishClientCapabilities",
    "canUseHostPaths",
];

/// Issue and replace have the same wire contract.
pub fn decode_issue_input(value: &Value) -> Result<AccessCredentialIssueInput> {
    let frame = record(value, "access credential issue input")?;
    shaped(frame, ISSUE_FIELDS, &["capabilityOwnerCredentialId"])?;
    let principal_kind = principal_kind(&frame["principalKind"])?;
    let capability_owner_credential_id = frame
        .get("capabilityOwnerCredentialId")
        .map(|v| string(v, "capabilityOwnerCredentialId", 128))
        .transpose()?;
    check_owner(principal_kind, capability_owner_credential_id.is_some())?;
    Ok(AccessCredentialIssueInput {
        principal_kind,
        principal_id: principal_id(&frame["principalId"])?,
        operation_grants: operation_grants(&frame["operationGrants"])?,
        can_publish_client_capabilities: boolean(&frame["canPublishClientCapabilities"])?,
        can_use_host_paths: boolean(&frame["canUseHostPaths"])?,
        capability_owner_credential_id,
    })
}

pub fn decode_prepare_input(value: &Value) -> Result<AccessCredentialPrepareInput> {
    let frame = record(value, "access credential prepare input")?;
    shaped(frame, ISSUE_FIELDS, &["bindClientInstance"])?;
    Ok(AccessCredentialPrepareInput {
        principal_kind: principal_kind(&frame["principalKind"])?,
        principal_id: principal_id(&frame["principalId"])?,
        operation_grants: operation_grants(&frame["operationGrants"])?,
        can_publish_client_capabilities: boolean(&frame["canPublishClientCapabilities"])?,
        can_use_host_paths: boolean(&frame["canUseHostPaths"])?,
        bind_client_instance: frame.get("bindClientInstance").map(boolean).transpose()?,
    })
}

pub fn decode_finalize_input(value: &Value) -> Result<()> {
    exact(record(value, "access credential finalize input")?, &[])
}

pub fn decode_finalize_result(value: &Value) -> Result<AccessCredentialFinalizeResult> {
    let frame = record(value, "access credential finalize result")?;
    exact(frame, &["reconnectRequired"])?;
    Ok(AccessCredentialFinalizeResult {
        reconnect_required: boolean(&frame["reconnectRequired"])?,
    })
}

pub fn decode_issue_result(value: &Value) -> Result<AccessCredentialIssueResult> {
    let frame = record(value, "access credential issue result")?;
    let mut required = ISSUE_FIELDS.to_vec();
    required.extend(["credentialId", "deliveryId"]);
    shaped(frame, &required, &["capabilityOwner"])?;
    let principal_kind = principal_kind(&frame["principalKind"])?;
    let capability_owner = frame
        .get("capabilityOwner")
        .map(owner_identity)
        .transpose()?;
    check_owner(principal_kind, capability_owner.is_some())?;
    Ok(AccessCredentialIssueResult {
        credential_id: string(&frame["credentialId"], "credentialId", 128)?,
        delivery_id: string(&frame["deliveryId"], "deliveryId", 128)?,
        principal_kind,
        principal_id: principal_id(&frame["principalId"])?,
        operation_grants: operation_grants(&frame["operationGrants"])?,
        can_publish_client_capabilities: boolean(&frame["canPublishClientCapabilities"])?,
        can_use_host_paths: boolean(&frame["canUseHostPaths"])?,
        capability_owner,
    })
}

pub fn decode_revoke_input(value: &Value) -> Result<AccessCredentialRevokeInput> {
    let frame = record(value, "access credential revoke input")?;
    exact(frame, &["credentialId"])?;
    Ok(AccessCredentialRevokeInput {
        credential_id: string(&frame["credentialId"], "credentialId", 128)?,
    })
}

pub fn decode_revoke_result(value: &Value) -> Result<AccessCredentialRevokeResult> {
    let frame = record(value, "access credential revoke result")?;
    exact(frame, &["credentialId", "revoked"])?;
    Ok(AccessCredentialRevokeResult {
        credential_id: string(&frame["credentialId"], "credentialId", 128)?,
        revoked: boolean(&frame["revoked"])?,
    })
}

fn principal_kind(value: &Value) -> Result<ManagedPrincipalKind> {
    match value.as_str() {
        Some("remote_owner") => Ok(ManagedPrincipalKind::RemoteOwner),
        Some("capability_provider") => Ok(ManagedPrincipalKind::CapabilityProvider),
        _ => Err(ProtocolError::invalid(
            "Invalid access credential principalKind",
        )),
    }
}

fn principal_id(value: &Value) -> Result<String> {
    value
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        })
        .map(str::to_owned)
        .ok_or_else(|| ProtocolError::invalid("Invalid access credential principalId"))
}

fn operation_grants(value: &Value) -> Result<Vec<String>> {
    let values = value
        .as_array()
        .filter(|a| a.len() <= ACCESS_CREDENTIAL_MAX_GRANTS)
        .ok_or_else(|| ProtocolError::invalid("Invalid access credential operation grants"))?;
    let grants = values
        .iter()
        .map(|v| string(v, "access credential operation grant", 128))
        .collect::<Result<Vec<_>>>()?;
    if grants.iter().collect::<HashSet<_>>().len() != grants.len() {
        return Err(ProtocolError::invalid(
            "Duplicate access credential operation grant",
        ));
    }
    Ok(grants)
}

fn owner_identity(value: &Value) -> Result<CapabilityOwnerIdentity> {
    let frame = record(value, "Client Capability owner identity")?;
    exact(frame, &["principalId", "clientInstanceId"])?;
    Ok(CapabilityOwnerIdentity {
        principal_id: principal_id(&frame["principalId"])?,
        client_instance_id: string(&frame["clientInstanceId"], "clientInstanceId", 128)?,
    })
}

fn check_owner(kind: ManagedPrincipalKind, present: bool) -> Result<()> {
    if present && kind != ManagedPrincipalKind::CapabilityProvider {
        return Err(ProtocolError::invalid(
            "Only a capability provider credential may declare a Client owner",
        ));
    }
    Ok(())
}

fn boolean(value: &Value) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| ProtocolError::invalid("Invalid access credential boolean"))
}
