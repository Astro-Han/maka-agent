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

use super::{Host, HostError};
use maka_client_capability::{
    BindingError, BindingMode, Endpoint, Identity, Registry, broker::Broker,
};
use maka_protocol::{
    Operation, OperationError, OperationErrorCode as Code, Outcome, ProtocolError, capability,
};
use maka_tools::{ClientTools, ToolRegistration};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// One ephemeral capability authority for all connections and live runs.
/// Publication lifetimes stay separate from durable execution facts.
#[derive(Default)]
pub(crate) struct Capabilities {
    pub(crate) registry: Arc<Mutex<Registry>>,
    pub(crate) broker: Arc<Broker>,
}

impl Capabilities {
    pub(super) fn attach(
        self: &Arc<Self>,
        connection_id: Uuid,
        identity: Identity,
        endpoint: Endpoint,
    ) -> Result<ConnectionLease, maka_client_capability::Error> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .attach(connection_id, identity, endpoint)?;
        Ok(ConnectionLease {
            capabilities: self.clone(),
            connection_id,
        })
    }

    /// Called under the Host's execution admission gate, after replay and Busy
    /// checks. A run owns its snapshot; replacement never edits that snapshot.
    pub(crate) fn bind_tools(
        &self,
        session_id: &str,
        connection_id: Option<Uuid>,
        mode: BindingMode,
        cwd: String,
        interactions: Arc<dyn maka_tools::ClientInteractions>,
    ) -> Result<Vec<ToolRegistration>, BindingError> {
        let snapshot = {
            let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            registry.bind_session(session_id, connection_id, mode)?;
            registry.snapshot(session_id)?
        };
        Ok(ClientTools::new(
            snapshot,
            self.registry.clone(),
            self.broker.clone(),
            cwd,
            interactions,
        )
        .registrations())
    }

    pub(crate) fn preview_tools(
        &self,
        session_id: Option<&str>,
        connection_id: Uuid,
        cwd: String,
        interactions: Arc<dyn maka_tools::ClientInteractions>,
    ) -> Result<Vec<ToolRegistration>, BindingError> {
        let snapshot = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .preview_bindings(session_id, Some(connection_id), BindingMode::Strict)?;
        Ok(ClientTools::new(
            snapshot,
            self.registry.clone(),
            self.broker.clone(),
            cwd,
            interactions,
        )
        .registrations())
    }

    pub(crate) fn bind_required_tools(
        &self,
        session_id: &str,
        connection_id: Uuid,
        required: &[&str],
        cwd: String,
        interactions: Arc<dyn maka_tools::ClientInteractions>,
    ) -> Result<Vec<ToolRegistration>, BindingError> {
        let snapshot = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .bind_required_tools(session_id, connection_id, required)?;
        Ok(ClientTools::new(
            snapshot,
            self.registry.clone(),
            self.broker.clone(),
            cwd,
            interactions,
        )
        .registrations())
    }

    pub(super) fn begin_drain(&self) {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .begin_drain();
    }

    pub(super) async fn shutdown(&self) {
        self.begin_drain();
        self.broker.shutdown().await;
    }
}

pub(super) struct ConnectionLease {
    capabilities: Arc<Capabilities>,
    connection_id: Uuid,
}

impl Drop for ConnectionLease {
    fn drop(&mut self) {
        self.capabilities
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .detach(self.connection_id);
    }
}

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ClientCapabilityReplace | Operation::ClientCapabilityUnregister
    )
}

pub(super) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::InternalFailure,
];

pub(super) fn decode_input(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    let normalized = match operation {
        Operation::ClientCapabilityReplace => {
            serde_json::to_value(capability::decode_replace_input(value)?)
        }
        Operation::ClientCapabilityUnregister => {
            serde_json::to_value(capability::decode_unregister_input(value)?)
        }
        _ => {
            return Err(ProtocolError::invalid(
                "Unknown Client Capability operation",
            ));
        }
    };
    normalized.map_err(|error| ProtocolError::invalid(error.to_string()))
}

pub(super) fn decode_output(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    if !supports(operation) {
        return Err(ProtocolError::invalid(
            "Unknown Client Capability operation",
        ));
    }
    serde_json::to_value(capability::decode_registration_result(value)?)
        .map_err(|error| ProtocolError::invalid(error.to_string()))
}

pub(super) async fn execute(
    host: &Host,
    connection_id: Uuid,
    operation: Operation,
    input: &Value,
) -> Result<Outcome, HostError> {
    let _admission = host.executions.lock_admission().await;
    if host.draining.is_cancelled() {
        return Ok(Outcome::failure(OperationError {
            code: Code::HostDraining,
            message: "Host is draining".into(),
        }));
    }
    let result = {
        let mut registry = host
            .capabilities
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match operation {
            Operation::ClientCapabilityReplace => {
                registry.replace(connection_id, capability::decode_replace_input(input)?)
            }
            Operation::ClientCapabilityUnregister => registry.unregister(
                connection_id,
                &capability::decode_unregister_input(input)?.registration_id,
            ),
            _ => unreachable!("validated Client Capability operation"),
        }
    };
    Ok(match result {
        Ok(output) => Outcome::success(decode_output(operation, &serde_json::to_value(output)?)?),
        Err(error) => Outcome::failure(OperationError {
            code: match error {
                maka_client_capability::Error::Draining => Code::HostDraining,
                maka_client_capability::Error::Unavailable => Code::OperationUnavailable,
                maka_client_capability::Error::Invalid(_) => Code::InvalidRequest,
            },
            message: error.to_string(),
        }),
    })
}
