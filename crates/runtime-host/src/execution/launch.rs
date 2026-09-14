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

use super::{ActiveRun, Executions, Result, execution_error, failure, requires_drain};
use maka_agent::RunInput;
use maka_protocol::{
    OperationErrorCode as Code,
    turn::{TurnQueryInput, TurnSnapshot},
};

impl Executions {
    /// Both hosted execution kinds retain the same admission and drain owner.
    /// The caller holds the shared admission gate through durable startup.
    pub(super) async fn launch(
        self: &std::sync::Arc<Self>,
        input: RunInput,
    ) -> Result<TurnSnapshot> {
        let invocation = input.invocation.clone();
        let cancellation = self.shutdown.child_token();
        if cancellation.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let running = self
            .engine
            .start(input, cancellation.clone())
            .await
            .map_err(|error| {
                if requires_drain(&error) {
                    self.begin_drain();
                }
                execution_error(error)
            })?;
        self.track(running);
        self.query(TurnQueryInput {
            session_id: invocation.session_id,
            turn_id: invocation.turn_id,
        })
        .await
    }

    pub(super) fn track(self: &std::sync::Arc<Self>, running: maka_agent::RunningInvocation) {
        self.active.lock().unwrap().insert(
            running.invocation().run_id.clone(),
            ActiveRun {
                invocation: running.invocation().clone(),
                tool_names: running.tool_names().clone(),
                cancellation: running.cancellation(),
                completed: tokio_util::sync::CancellationToken::new(),
            },
        );
        self.workers.spawn(self.clone().drive(running));
    }
}
