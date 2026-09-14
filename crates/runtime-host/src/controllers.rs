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

use crate::shell::ShellHandle;
use maka_protocol::{
    Outcome,
    resource::{ControllerControlInput, ControllerIdentity},
};
use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
};
use uuid::Uuid;

#[derive(Default, Clone)]
pub(crate) struct Controllers(Arc<Mutex<State>>);

#[derive(Default)]
pub(super) struct State {
    pub connections: HashSet<Uuid>,
    pub leases: Vec<Lease>,
    replays: VecDeque<Replay>,
}
pub(super) struct Lease {
    pub connection: Uuid,
    pub identity: ControllerIdentity,
    pub next_sequence: u64,
    pub handle: ShellHandle,
}
struct Replay {
    connection: Uuid,
    input: ControllerControlInput,
    outcome: Outcome,
}

pub(crate) struct Connection {
    controllers: Controllers,
    id: Uuid,
}
impl Controllers {
    pub(crate) fn connection(&self, id: Uuid) -> Connection {
        self.lock().connections.insert(id);
        Connection {
            controllers: self.clone(),
            id,
        }
    }
    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        let mut state = self.0.lock().unwrap();
        state.leases.retain(
            |lease| matches!(lease.handle.latest(), Some(Ok(record)) if record.state.active()),
        );
        state
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        let mut state = self.controllers.lock();
        state.connections.remove(&self.id);
        state.leases.retain(|lease| lease.connection != self.id);
        state.replays.retain(|replay| replay.connection != self.id);
    }
}
impl State {
    pub fn resource(&self, identity: &ControllerIdentity) -> Option<usize> {
        self.leases
            .iter()
            .position(|lease| same_resource(&lease.identity, identity))
    }
    pub fn replay(&self, connection: Uuid, input: &ControllerControlInput) -> Option<Outcome> {
        self.replays
            .iter()
            .find(|replay| {
                replay.connection == connection
                    && replay.input.sequence == input.sequence
                    && same_identity(&replay.input.identity(), &input.identity())
            })
            .map(|replay| {
                if replay.input.control == input.control {
                    replay.outcome.clone()
                } else {
                    conflict("Controller sequence was retried with different input")
                }
            })
    }
    pub fn forget(&mut self, connection: Uuid, identity: &ControllerIdentity) {
        self.replays.retain(|replay| {
            replay.connection != connection || !same_identity(&replay.input.identity(), identity)
        });
    }
    pub fn remember(&mut self, connection: Uuid, input: ControllerControlInput, outcome: Outcome) {
        // Disconnect may have happened while the accepted native input drained.
        if !self.connections.contains(&connection) {
            return;
        }
        self.forget(connection, &input.identity());
        self.replays.push_back(Replay {
            connection,
            input,
            outcome,
        });
        if self.replays.len() > 128 {
            self.replays.pop_front();
        }
    }
}
pub(super) fn same_resource(a: &ControllerIdentity, b: &ControllerIdentity) -> bool {
    a.session_id == b.session_id && a.resource_ref == b.resource_ref
}
pub(super) fn same_identity(a: &ControllerIdentity, b: &ControllerIdentity) -> bool {
    same_resource(a, b) && a.controller_id == b.controller_id
}

pub(crate) fn conflict(message: &str) -> Outcome {
    Outcome::Failure {
        error: maka_protocol::OperationError {
            code: maka_protocol::OperationErrorCode::OperationConflict,
            message: message.into(),
        },
    }
}
