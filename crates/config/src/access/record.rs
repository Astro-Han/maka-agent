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

use crate::{ConfigError, Result};
use maka_runtime::access::{CapabilityOwnerIdentity, ManagedPrincipalKind};
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CredentialState {
    Active {
        client_instance_id: Option<String>,
    },
    Pending {
        expires_at: u64,
        bind_client_instance: bool,
    },
    Revoked {
        revoked_at: String,
    },
}
/// Raw grants remain open so older builds cannot erase future grants.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessCredential {
    pub credential_id: String,
    pub credential_hash: String,
    pub principal_id: String,
    pub principal_kind: ManagedPrincipalKind,
    pub grants: Vec<String>,
    pub can_publish_client_capabilities: bool,
    pub can_use_host_paths: bool,
    pub created_at: String,
    pub state: CredentialState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_owner: Option<CapabilityOwnerIdentity>,
}
impl AccessCredential {
    pub fn client_instance_id(&self) -> Option<&str> {
        match &self.state {
            CredentialState::Active { client_instance_id } => client_instance_id.as_deref(),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessCreateMode {
    Issue,
    Replace,
    Prepare,
}
pub struct AccessChange<T> {
    pub value: T,
    /// Returned only after the enclosing transaction commits.
    pub revoked: Vec<String>,
}
const MAX_ACCESS_BYTES: usize = 512 * 1024;
pub(super) fn invalid(message: &str) -> ConfigError {
    ConfigError::Invalid(message.into())
}
pub(super) fn valid_client(value: &str) -> bool {
    !value.is_empty() && value.encode_utf16().count() <= 128
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
}
pub(super) fn validate(value: &AccessCredential) -> Result<()> {
    let date = |s: &str| !s.is_empty() && s.len() <= 128;
    let state_valid = match &value.state {
        CredentialState::Active { client_instance_id } => {
            client_instance_id.as_deref().is_none_or(valid_client)
                && (client_instance_id.is_none()
                    || value.principal_kind == ManagedPrincipalKind::RemoteOwner)
        }
        CredentialState::Pending { expires_at, .. } => {
            // JavaScript Date's supported Unix millisecond range.
            *expires_at <= 8_640_000_000_000_000
                && value.principal_kind == ManagedPrincipalKind::RemoteOwner
                && value
                    .grants
                    .iter()
                    .any(|g| g == "access.credential.finalize")
        }
        CredentialState::Revoked { revoked_at } => date(revoked_at),
    };
    if !valid_id(&value.credential_id)
        || !valid_id(&value.principal_id)
        || value.credential_hash.len() != 64
        || !value
            .credential_hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || value.grants.len() > 256
        || value
            .grants
            .iter()
            .any(|g| g.is_empty() || g.encode_utf16().count() > 128)
        || value
            .grants
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != value.grants.len()
        || !date(&value.created_at)
        || !state_valid
        || value.capability_owner.as_ref().is_some_and(|o| {
            value.principal_kind != ManagedPrincipalKind::CapabilityProvider
                || !valid_id(&o.principal_id)
                || !valid_client(&o.client_instance_id)
        })
    {
        return Err(invalid("invalid access credential record"));
    }
    Ok(())
}
pub(super) async fn load(tx: &mut SqliteConnection) -> Result<Vec<AccessCredential>> {
    let bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(length(CAST(document AS BLOB))), 0) FROM access_credentials",
    )
    .fetch_one(&mut *tx)
    .await?;
    if bytes > MAX_ACCESS_BYTES as i64 {
        return Err(invalid("access credential capacity exceeded"));
    }
    let documents: Vec<String> =
        sqlx::query_scalar("SELECT document FROM access_credentials ORDER BY credential_id")
            .fetch_all(tx)
            .await?;
    documents
        .iter()
        .map(|s| {
            let c = serde_json::from_str(s)?;
            validate(&c)?;
            Ok(c)
        })
        .collect()
}
/// Replaces the bounded private document set inside the caller's immediate transaction.
pub(super) async fn save(tx: &mut SqliteConnection, records: &[AccessCredential]) -> Result<()> {
    let documents: Vec<String> = records
        .iter()
        .map(|c| {
            validate(c)?;
            Ok(serde_json::to_string(c)?)
        })
        .collect::<Result<_>>()?;
    if documents.iter().map(String::len).sum::<usize>() > MAX_ACCESS_BYTES {
        return Err(invalid("access credential capacity exceeded"));
    }
    sqlx::query("DELETE FROM access_credentials")
        .execute(&mut *tx)
        .await?;
    for document in documents {
        sqlx::query("INSERT INTO access_credentials (document) VALUES (?)")
            .bind(document)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}
pub(super) fn same_principal(a: &AccessCredential, b: &AccessCredential) -> bool {
    a.principal_kind == b.principal_kind && a.principal_id == b.principal_id
}
