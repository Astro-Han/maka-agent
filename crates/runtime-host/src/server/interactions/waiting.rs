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

use super::{Interactions, failure};
use maka_protocol::{OperationError, OperationErrorCode as Code};
use maka_runtime::interaction::{ClosureReason, InteractionOutcome};
use tokio_util::sync::CancellationToken;

impl Interactions {
    /// Execution/broker owns the waiter through cancellation and withdrawal.
    /// A prior user answer or whole-Run closure remains the canonical winner.
    pub(super) async fn wait_for_outcome(
        &self,
        request_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<InteractionOutcome, OperationError> {
        // Subscribe before reading: a commit before subscription is already
        // visible, and one after reading always wakes this waiter.
        let mut commits = self.log.subscribe_commits();
        loop {
            // Stop holds this gate through the durable closure and Run cancellation.
            // Do not release a producer in the interval between those two steps.
            let gate = self.admission.lock().await;
            let current = self
                .log
                .interaction(request_id)
                .await
                .map_err(|error| self.store_failure(error))?
                .ok_or_else(|| self.missing_outcome())?;
            if let Some(outcome) = current.outcome {
                return Ok(outcome);
            }
            drop(gate);
            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => {},
                _ = cancellation.cancelled() => {},
                changed = commits.changed() => {
                    if changed.is_ok() { continue; }
                    self.shutdown.cancel();
                }
            }
            let _gate = self.admission.lock().await;
            return self
                .log
                .commit_interaction_outcome(
                    request_id,
                    InteractionOutcome::Closure {
                        reason: ClosureReason::ProducerCancelled,
                        committed_at: self.timestamp()?,
                    },
                )
                .await
                .map_err(|error| self.store_failure(error))?
                .record
                .outcome
                .ok_or_else(|| self.missing_outcome());
        }
    }

    fn missing_outcome(&self) -> OperationError {
        self.shutdown.cancel();
        failure(
            Code::InternalFailure,
            "Canonical interaction fact disappeared",
        )
    }
}
