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

use super::Host;
use maka_protocol::{
    OperationError, OperationErrorCode,
    host::{RetirementInput, RetirementResult},
};

impl Host {
    pub(super) async fn prepare_retirement(
        &self,
        input: RetirementInput,
    ) -> Result<RetirementResult, OperationError> {
        let _admission = self.executions.lock_admission().await;
        let mut retiring = self
            .handshake_gate
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if input.expected_host_epoch != self.epoch || *retiring || self.draining.is_cancelled() {
            return Err(OperationError {
                code: OperationErrorCode::OperationConflict,
                message: "Host lifetime changed or retirement already began".into(),
            });
        }
        if !input.allow_interrupt_active_tasks && self.activity().blocks_retirement(1) {
            // Until durable step handoff is installed, this Host does not advertise
            // it and never interprets permission to cooperate as permission to kill.
            return Ok(RetirementResult::ActiveTasks);
        }
        *retiring = true;
        self.draining.cancel();
        drop(retiring);
        drop(_admission);
        self.record_diagnostic("Host retirement committed; draining owned work");
        // The accepted request token protects this reply until flushed/abandoned.
        // Root release still waits for execution, native I/O and storage cleanup.
        Ok(RetirementResult::Prepared {
            pid: std::num::NonZeroU32::new(std::process::id()).expect("process PID"),
        })
    }
}
