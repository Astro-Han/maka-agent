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

use crate::{ProtocolError, Result};
use maka_plugins::remote::{ClientIdentity, Target};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged, rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum RemoteBinding {
    /// Plugin frontend and backend must originate from the same package bytes.
    Client {
        client: ClientIdentity,
        method: String,
        session_id: Option<String>,
    },
    /// An authenticated application/CLI calls a plugin without loading its frontend.
    Package {
        package_id: String,
        method: String,
        session_id: Option<String>,
    },
}
impl RemoteBinding {
    pub fn method(&self) -> &str {
        match self {
            Self::Client { method, .. } | Self::Package { method, .. } => method,
        }
    }
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Client { session_id, .. } | Self::Package { session_id, .. } => {
                session_id.as_deref()
            }
        }
    }
    pub fn package_id(&self) -> &str {
        match self {
            Self::Client { client, .. } => &client.extension_id,
            Self::Package { package_id, .. } => package_id,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RemoteRequest {
    OpenDocument,
    Bind {
        binding: RemoteBinding,
    },
    Call {
        binding: RemoteBinding,
        target: Target,
        document: Uuid,
        input: Value,
    },
    Open {
        binding: RemoteBinding,
        target: Target,
        document: Uuid,
        input: Value,
    },
    Next {
        document: Uuid,
        stream: Uuid,
    },
    Close {
        document: Uuid,
        stream: Uuid,
    },
    CloseDocument {
        document: Uuid,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteKind {
    Method,
    Stream,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RemoteResult {
    Document { document: Uuid },
    Bound { target: Target, handler: RemoteKind },
    Value { value: Value },
    Opened { stream: Uuid },
    Item { item: Value },
    Pending,
    End,
    Closed,
}
impl RemoteRequest {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Bind { binding } => validate_binding(binding),
            Self::Call {
                binding,
                target,
                input,
                ..
            }
            | Self::Open {
                binding,
                target,
                input,
                ..
            } => {
                validate_binding(binding)?;
                validate_target(target)?;
                payload(input)
            }
            Self::OpenDocument
            | Self::Next { .. }
            | Self::Close { .. }
            | Self::CloseDocument { .. } => Ok(()),
        }
    }
}
pub fn validate_remote_result(value: &Value) -> Result<Value> {
    let result: RemoteResult = serde_json::from_value(value.clone())
        .map_err(|error| ProtocolError::invalid(error.to_string()))?;
    match &result {
        RemoteResult::Bound { target, .. } => validate_target(target)?,
        RemoteResult::Value { value } | RemoteResult::Item { item: value } => payload(value)?,
        _ => {}
    }
    Ok(value.clone())
}
fn validate_target(target: &Target) -> Result<()> {
    super::client::identity(&target.entry_id)?;
    super::client::activation_id(&target.activation)
}
pub(super) fn validate_binding(binding: &RemoteBinding) -> Result<()> {
    match binding {
        RemoteBinding::Client { client, .. } => validate_client(client)?,
        RemoteBinding::Package { package_id, .. } => super::client::identity(package_id)?,
    }
    super::client::identity(binding.method())?;
    if let Some(session) = binding.session_id() {
        maka_runtime::interaction::entity_id(session).map_err(ProtocolError::invalid)?;
    }
    Ok(())
}
pub(super) fn validate_client(client: &ClientIdentity) -> Result<()> {
    super::client::identity(&client.entry_id)?;
    super::client::identity(&client.extension_id)?;
    super::client::activation_id(&client.activation)?;
    super::client::digest(&client.content_digest)?;
    super::client::digest(&client.client_digest)
}
fn payload(value: &Value) -> Result<()> {
    maka_plugins::remote::validate_payload(value)
        .map_err(|error| ProtocolError::invalid(error.to_string()))
}
