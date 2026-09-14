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

use super::{Host, configuration, delivery, internal, invalid, publish, timestamp};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use maka_config::access::{AccessCreateMode, AccessCredential, CredentialState};
use maka_protocol::{Operation, OperationError, OperationErrorCode};
use maka_runtime::access::{
    AccessCredentialIssueInput, AccessCredentialIssueResult, ManagedPrincipalKind,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;

pub(super) async fn execute(
    host: &Host,
    operation: Operation,
    value: &Value,
) -> Result<AccessCredentialIssueResult, OperationError> {
    let (input, mode, state) = if operation == Operation::AccessCredentialPrepare {
        let input = maka_protocol::access::decode_prepare_input(value).map_err(invalid)?;
        let bind_client_instance = input.bind_client_instance.unwrap_or(false);
        let expires_at = configuration::now()
            .map_err(configuration::failure)?
            .checked_add(15 * 60 * 1000)
            .ok_or_else(|| internal("Pairing expiry exceeds clock range"))?;
        (
            AccessCredentialIssueInput {
                principal_kind: input.principal_kind,
                principal_id: input.principal_id,
                operation_grants: input.operation_grants,
                can_publish_client_capabilities: input.can_publish_client_capabilities,
                can_use_host_paths: input.can_use_host_paths,
                capability_owner_credential_id: None,
            },
            AccessCreateMode::Prepare,
            CredentialState::Pending {
                expires_at,
                bind_client_instance,
            },
        )
    } else {
        (
            maka_protocol::access::decode_issue_input(value).map_err(invalid)?,
            if operation == Operation::AccessCredentialReplace {
                AccessCreateMode::Replace
            } else {
                AccessCreateMode::Issue
            },
            CredentialState::Active {
                client_instance_id: None,
            },
        )
    };
    let mut grants = vec![Operation::HostStatus];
    for name in input.operation_grants {
        let operation = name.parse::<Operation>().map_err(invalid)?;
        if !operation.allows_remote_owner() {
            return Err(invalid("Operation grant is local-owner only"));
        }
        if !grants.contains(&operation) {
            grants.push(operation);
        }
    }
    if input.principal_kind == ManagedPrincipalKind::CapabilityProvider
        && (!input.can_publish_client_capabilities
            || input.can_use_host_paths
            || grants.len() != super::super::authority::CAPABILITY_PROVIDER_GRANTS.len()
            || grants
                .iter()
                .any(|grant| !super::super::authority::CAPABILITY_PROVIDER_GRANTS.contains(grant)))
    {
        return Err(invalid(
            "A capability provider requires exactly publication grants and no Host path authority",
        ));
    }
    if matches!(state, CredentialState::Pending { .. })
        && (input.principal_kind != ManagedPrincipalKind::RemoteOwner
            || !grants.contains(&Operation::AccessCredentialFinalize))
    {
        return Err(invalid(
            "A pairing candidate must be a remote owner that can finalize its pairing",
        ));
    }
    let grants = grants
        .into_iter()
        .map(|operation| operation.as_str().into())
        .collect();
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| internal("Cannot generate access credential"))?;
    let secret = format!("maka_rh_{}", URL_SAFE_NO_PAD.encode(bytes));
    let credential_id = uuid::Uuid::new_v4().to_string();
    let credential = AccessCredential {
        credential_id: credential_id.clone(),
        credential_hash: format!("{:x}", Sha256::digest(secret.as_bytes())),
        principal_id: input.principal_id,
        principal_kind: input.principal_kind,
        grants,
        can_publish_client_capabilities: input.can_publish_client_capabilities,
        can_use_host_paths: input.can_use_host_paths,
        created_at: timestamp()?,
        state,
        capability_owner: None,
    };
    host.root
        .validate_current()
        .map_err(|_| internal("Root authority changed"))?;
    let control = host.control_directory().to_owned();
    let delivered_id = credential_id.clone();
    let delivery = tokio::task::spawn_blocking(move || {
        delivery::Delivery::create(&control, &delivered_id, &secret)
    })
    .await
    .map_err(|_| internal("Access delivery worker failed"))?
    .map_err(|_| OperationError {
        code: OperationErrorCode::PersistenceFailed,
        message: "Cannot create private access delivery".into(),
    })?;
    let change = host
        .configuration
        .create_access_credential(credential, mode, input.capability_owner_credential_id)
        .await
        .map_err(configuration::failure)?;
    publish(host, change.revoked);
    let credential = change.value;
    let delivery_id = delivery.id().to_owned();
    let draining = host.draining.clone();
    host.requests.spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(60)) => {},
            _ = draining.cancelled() => {},
        }
        drop(delivery);
    });
    Ok(AccessCredentialIssueResult {
        credential_id,
        delivery_id,
        principal_kind: credential.principal_kind,
        principal_id: credential.principal_id,
        operation_grants: credential.grants,
        can_publish_client_capabilities: credential.can_publish_client_capabilities,
        can_use_host_paths: credential.can_use_host_paths,
        capability_owner: credential.capability_owner,
    })
}
