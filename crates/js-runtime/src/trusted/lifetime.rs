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

use super::{Command, TrustedError, failed};
use deno_core::v8;
use std::sync::{Mutex, OnceLock};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(super) struct Health {
    pub isolate: OnceLock<v8::IsolateHandle>,
    pub failure: Mutex<Option<String>>,
}

impl Health {
    pub fn fail(&self, message: impl Into<String>) {
        self.failure
            .lock()
            .unwrap()
            .get_or_insert_with(|| message.into());
        if let Some(isolate) = self.isolate.get() {
            isolate.terminate_execution();
        }
    }

    pub fn error(&self) -> TrustedError {
        failed(
            self.failure
                .lock()
                .unwrap()
                .as_deref()
                .unwrap_or("worker closed"),
        )
    }
}

/// Dropping the caller must abort pending HTTP, not merely unblock op_emit.
/// Sending Cancel does not acknowledge cleanup or release the actor's permit.
pub(super) struct ModelCancellation {
    id: Option<u32>,
    commands: mpsc::UnboundedSender<Command>,
    token: CancellationToken,
}

impl ModelCancellation {
    pub fn new(
        id: u32,
        commands: mpsc::UnboundedSender<Command>,
        token: CancellationToken,
    ) -> Self {
        Self {
            id: Some(id),
            commands,
            token,
        }
    }

    pub fn disarm(&mut self) {
        self.id = None;
    }

    pub fn cancel(&mut self) {
        if let Some(id) = self.id.take() {
            self.token.cancel();
            let _ = self.commands.send(Command::Cancel(id));
        }
    }
}

impl Drop for ModelCancellation {
    fn drop(&mut self) {
        self.cancel();
    }
}
