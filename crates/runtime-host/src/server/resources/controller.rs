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

use super::{
    Host, failure,
    mutation::{active_session, fault},
};
use crate::controllers::{self as state, conflict};
use crate::shell::{ControlInput, ShellHandle};
use maka_presentation::shell::RESOURCE_REF_PREFIX;
use maka_protocol::{Operation, OperationErrorCode as Code, Outcome, resource::*};
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(super) async fn acquire(
    host: &Host,
    connection: Uuid,
    identity: ControllerIdentity,
) -> Outcome {
    let handle = match live(host, &identity).await {
        Ok(handle) => handle,
        Err(outcome) => return outcome,
    };
    let _gate = handle.control_gate.lock().await;
    let _admission = host.executions.lock_admission().await;
    if let Err(outcome) = authorize_control(host, &identity, &handle).await {
        return outcome;
    }
    let mut state = host.controllers.lock();
    if !state.connections.contains(&connection) {
        return conflict("Controller connection closed");
    }
    if !active(&handle) {
        return conflict("Only an active PTY Runtime Resource can be controlled");
    }
    if state.leases.iter().any(|lease| {
        lease.connection == connection
            && lease.identity.controller_id == identity.controller_id
            && !state::same_resource(&lease.identity, &identity)
    }) {
        return conflict("Controller identity is already bound to another Runtime Resource");
    }
    let next_sequence = match state.resource(&identity) {
        Some(index) => {
            let lease = &state.leases[index];
            if lease.connection != connection || !state::same_identity(&lease.identity, &identity) {
                return conflict("Runtime Resource already has a connected controller");
            }
            lease.next_sequence
        }
        None => {
            state.leases.push(state::Lease {
                connection,
                identity: identity.clone(),
                next_sequence: 1,
                handle: handle.clone(),
            });
            1
        }
    };
    let replay = handle.replay().expect("active PTY has replay");
    let mut output = ControllerAcquireResult {
        controller_id: identity.controller_id,
        next_sequence,
        pty: PtySnapshot {
            session_id: identity.session_id,
            resource_ref: identity.resource_ref,
            sequence: replay.sequence,
            buffer: replay.buffer,
            size: replay.size,
        },
    };
    drop(state);
    loop {
        let value = match serde_json::to_value(&output) {
            Ok(value) => value,
            Err(error) => return fault(host, error),
        };
        if serde_json::to_vec(&value).is_ok_and(|bytes| bytes.len() <= MAX_ACQUIRE_BYTES) {
            return checked(host, Operation::RuntimeResourceControllerAcquire, value);
        }
        let count = output.pty.buffer.chars().count();
        if count == 0 {
            return fault(host, "PTY metadata exceeds acquire limit");
        }
        output.pty.buffer = output.pty.buffer.chars().skip(count.div_ceil(2)).collect();
    }
}

pub(super) async fn control(
    host: &Host,
    connection: Uuid,
    input: ControllerControlInput,
) -> Outcome {
    if let Some(outcome) = host.controllers.lock().replay(connection, &input) {
        return outcome;
    }
    let identity = input.identity();
    if resource_id(&identity).is_none() {
        return failure(Code::InvalidRequest, "Runtime Resource ref is unsupported");
    }
    let handle = {
        let state = host.controllers.lock();
        state
            .resource(&identity)
            .map(|index| state.leases[index].handle.clone())
    };
    let Some(handle) = handle else {
        if let Err(outcome) = active_session(host, &identity.session_id).await {
            return outcome;
        }
        return conflict("Runtime Resource controller is not held by this connection");
    };
    let _gate = handle.control_gate.lock().await;
    let admission = host.executions.lock_admission().await;
    {
        let state = host.controllers.lock();
        if let Some(outcome) = state.replay(connection, &input) {
            return outcome;
        }
    }
    if let Err(outcome) = authorize_control(host, &identity, &handle).await {
        return outcome;
    }
    {
        let state = host.controllers.lock();
        if !state.connections.contains(&connection) {
            return conflict("Controller connection closed");
        }
        let Some(index) = state.resource(&identity) else {
            return conflict("Runtime Resource controller is not held by this connection");
        };
        let lease = &state.leases[index];
        if lease.connection != connection || !state::same_identity(&lease.identity, &identity) {
            return conflict("Runtime Resource controller is not held by this connection");
        }
        if input.sequence != lease.next_sequence {
            return conflict("Runtime Resource controller sequence is out of order");
        }
    }
    let (bytes, size) = input.control.parts().expect("decoded control");
    let pending = handle.enqueue_control(
        ControlInput::Raw(bytes.to_owned()),
        size,
        CancellationToken::new(),
    );
    // Queue acceptance and permission updates share one admission boundary.
    // Native I/O retains only the per-resource gate, including across disconnect.
    drop(admission);
    let result = match pending {
        Ok(receipt) => receipt.await,
        Err(error) => Err(error),
    };
    let outcome = match result {
        Ok(_) => encoded(
            host,
            Operation::RuntimeResourceControllerControl,
            ControllerControlResult {
                controller_id: identity.controller_id.clone(),
                sequence: input.sequence,
            },
        ),
        Err(error) => {
            handle.stop();
            if error.accepted_bytes.is_none() || error.resized.is_none() {
                fault(host, error) // Only an unknown outcome invalidates Host admission.
            } else {
                conflict("Runtime Resource PTY control is closed while the process is stopping")
            }
        }
    };
    let mut state = host.controllers.lock();
    if let Some(index) = state.resource(&identity)
        && state.leases[index].connection == connection
        && state::same_identity(&state.leases[index].identity, &identity)
    {
        if input.sequence == MAX_CONTROL_SEQUENCE || !matches!(outcome, Outcome::Success { .. }) {
            state.leases.remove(index);
        } else {
            state.leases[index].next_sequence += 1;
        }
    }
    state.remember(connection, input, outcome.clone());
    outcome
}

pub(super) async fn release(
    host: &Host,
    connection: Uuid,
    identity: ControllerIdentity,
) -> Outcome {
    if resource_id(&identity).is_none() {
        return failure(Code::InvalidRequest, "Runtime Resource ref is unsupported");
    }
    let handle = {
        let state = host.controllers.lock();
        state
            .resource(&identity)
            .map(|index| state.leases[index].handle.clone())
    };
    // An active predecessor's accepted control must settle before release.
    let _gate = match &handle {
        Some(handle) => Some(handle.control_gate.lock().await),
        None => None,
    };
    let mut state = host.controllers.lock();
    let released = if let Some(index) = state.resource(&identity) {
        let lease = &state.leases[index];
        if lease.connection != connection || !state::same_identity(&lease.identity, &identity) {
            return conflict("Runtime Resource controller is held by another connection");
        }
        state.leases.remove(index);
        state.forget(connection, &identity);
        true
    } else {
        false
    };
    encoded(
        host,
        Operation::RuntimeResourceControllerRelease,
        ControllerReleaseResult {
            controller_id: identity.controller_id,
            released,
        },
    )
}

async fn live(host: &Host, identity: &ControllerIdentity) -> Result<ShellHandle, Outcome> {
    let id = resource_id(identity)
        .ok_or_else(|| failure(Code::InvalidRequest, "Runtime Resource ref is unsupported"))?;
    if let Some(handle) = host.shells.get(&identity.session_id, id) {
        if active(&handle) {
            return Ok(handle);
        }
        active_session(host, &identity.session_id).await?;
        return Err(conflict(
            "Only an active PTY Runtime Resource can be controlled",
        ));
    }
    active_session(host, &identity.session_id).await?;
    match host.log.read_shell_run(&identity.session_id, id).await {
        Ok(Some(_)) => Err(conflict("Runtime Resource PTY is no longer available")),
        Ok(None) => Err(failure(
            Code::NotFound,
            "Runtime Resource was not found in this Session",
        )),
        Err(error) => Err(fault(host, error)),
    }
}

// Caller holds execution admission. Reading a Session ID alone does not grant
// new input to a process launched under an obsolete permission boundary.
async fn authorize_control(
    host: &Host,
    identity: &ControllerIdentity,
    handle: &ShellHandle,
) -> Result<(), Outcome> {
    let session = active_session(host, &identity.session_id).await?;
    let Some(Ok(record)) = handle.latest() else {
        return Err(conflict("Runtime Resource PTY is not available"));
    };
    if session.configuration.boundary_revision != record.permissions.boundary_revision {
        return Err(conflict(
            "PTY launch permissions no longer match this Session; start a new terminal",
        ));
    }
    Ok(())
}
fn active(handle: &ShellHandle) -> bool {
    matches!(handle.latest(), Some(Ok(record)) if record.state.active() && record.output.is_pty())
}
fn resource_id(identity: &ControllerIdentity) -> Option<&str> {
    identity
        .resource_ref
        .strip_prefix(RESOURCE_REF_PREFIX)
        .filter(|id| maka_runtime::interaction::entity_id(id).is_ok())
}
fn encoded(host: &Host, operation: Operation, output: impl Serialize) -> Outcome {
    match serde_json::to_value(output) {
        Ok(value) => checked(host, operation, value),
        Err(error) => fault(host, error),
    }
}
fn checked(host: &Host, operation: Operation, value: Value) -> Outcome {
    match validate_controller_output(operation, &value) {
        Ok(()) => Outcome::success(value),
        Err(error) => fault(host, error),
    }
}
