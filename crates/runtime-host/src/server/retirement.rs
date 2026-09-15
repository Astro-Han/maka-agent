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
    OperationError, OperationErrorCode as Code,
    host::{RetirementInput, RetirementResult},
};
use std::{collections::BTreeSet, time::Duration};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Phase {
    #[default]
    Ready,
    Preparing,
    Retiring,
}

/// Before the first commit, dropping the request releases its scheduling fence.
/// Afterwards cleanup and root release must finish even if the requester leaves.
struct Preparation<'a> {
    host: &'a Host,
    committed: bool,
}
impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        *self
            .host
            .retirement
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = if self.committed {
            Phase::Retiring
        } else {
            Phase::Ready
        };
        if self.committed {
            self.host.draining.cancel();
        } else {
            self.host.executions.request_handoff_recovery();
        }
    }
}

impl Host {
    pub(super) async fn prepare_retirement(
        &self,
        input: RetirementInput,
    ) -> Result<RetirementResult, OperationError> {
        let admission = self.executions.lock_admission().await;
        let preparation = {
            let mut phase = self.retirement.lock().unwrap_or_else(|e| e.into_inner());
            if input.expected_host_epoch != self.epoch
                || *phase != Phase::Ready
                || self.draining.is_cancelled()
            {
                return Err(failure(
                    Code::OperationConflict,
                    "Host lifetime changed or retirement already began",
                ));
            }
            let activity = self.activity();
            if input.allow_interrupt_active_tasks || !activity.blocks_retirement(1) {
                *phase = Phase::Retiring;
                self.draining.cancel();
                return Ok(prepared());
            }
            if input.allow_cooperative_handoff != Some(true) || activity.blocks_cooperation(1) {
                return Ok(RetirementResult::ActiveTasks);
            }
            *phase = Phase::Preparing;
            Preparation {
                host: self,
                committed: false,
            }
        };
        drop(admission);
        self.cooperate(preparation).await
    }

    async fn cooperate(
        &self,
        mut preparation: Preparation<'_>,
    ) -> Result<RetirementResult, OperationError> {
        let handoff_id = uuid::Uuid::new_v4().to_string();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut held: Vec<(crate::execution::CooperativeRun, maka_agent::HeldHandoff)> = Vec::new();
        let mut reserved = BTreeSet::new();
        loop {
            let admission = self.executions.lock_admission().await;
            if self.draining.is_cancelled() || self.activity().blocks_cooperation(1) {
                return Ok(RetirementResult::ActiveTasks);
            }
            let Some(runs) = self.executions.cooperative_runs() else {
                return Ok(RetirementResult::ActiveTasks);
            };
            let mut waiting = Vec::new();
            for run in runs {
                if reserved.contains(&run.invocation.run_id) {
                    continue;
                }
                let intent = maka_runtime::handoff::HandoffIntent {
                    handoff_id: handoff_id.clone(),
                    host_epoch: self.epoch.clone(),
                    root_run_id: run.gate.root_run_id().into(),
                    successor_run_id: uuid::Uuid::new_v4().to_string(),
                    successor_invocation_id: uuid::Uuid::new_v4().to_string(),
                    claim_id: uuid::Uuid::new_v4().to_string(),
                };
                let Ok(reservation) = run.gate.reserve(intent) else {
                    return Ok(RetirementResult::ActiveTasks);
                };
                reserved.insert(run.invocation.run_id.clone());
                waiting.push(async move {
                    match reservation.ready().await {
                        Some(held) => Ok(Some((run, held))),
                        None if run.completed.is_cancelled() => Ok(None),
                        None => Err(()),
                    }
                });
            }
            if waiting.is_empty() {
                let mut seals = Vec::new();
                let mut cleanup = Vec::new();
                {
                    // Pair the last activity check with the connection admission
                    // fence. A callback either retains command residency here or
                    // observes Retiring before it can enter the dispatcher.
                    let mut phase = self.retirement.lock().unwrap_or_else(|e| e.into_inner());
                    if self.draining.is_cancelled() || self.activity().blocks_cooperation(1) {
                        return Ok(RetirementResult::ActiveTasks);
                    }
                    // All admitted owners are held or have completed. Commit is
                    // synchronous under admission; cancellation is the only race.
                    for (run, held) in held {
                        if let Some(seal) = held.commit() {
                            preparation.committed = true;
                            seals.push(seal);
                        }
                        cleanup.push(run.completed);
                    }
                    // Even an empty set can retire: all work completed during preparation.
                    preparation.committed = true;
                    *phase = Phase::Retiring;
                }
                drop(admission);
                let sealed =
                    futures_util::future::join_all(seals.into_iter().map(|seal| seal.wait())).await;
                futures_util::future::join_all(cleanup.iter().map(|done| done.cancelled())).await;
                if sealed.iter().any(Option::is_none) {
                    return Err(failure(
                        Code::HostDraining,
                        "Cooperative seal was not confirmed; Host is draining for recovery",
                    ));
                }
                self.record_diagnostic("Host cooperative retirement sealed; releasing owned work");
                return Ok(prepared());
            }
            drop(admission);
            match tokio::time::timeout_at(deadline, futures_util::future::try_join_all(waiting))
                .await
            {
                Ok(Ok(ready)) => held.extend(ready.into_iter().flatten()),
                _ => return Ok(RetirementResult::ActiveTasks),
            }
        }
    }
}

fn prepared() -> RetirementResult {
    RetirementResult::Prepared {
        pid: std::num::NonZeroU32::new(std::process::id()).expect("process PID"),
    }
}

pub(super) fn allows_preparing(operation: maka_protocol::Operation) -> bool {
    use maka_protocol::{Operation::*, operation::OperationMode};
    operation.mode() == OperationMode::Query
        || matches!(
            operation,
            InteractionAnswer
                | TurnStop
                | TurnInterrupt
                | RuntimeResourceStop
                | RuntimeResourceControllerAcquire
                | RuntimeResourceControllerControl
                | RuntimeResourceControllerRelease
                | OauthLoginCancel
                | SubscriptionOpen
                | SubscriptionClose
                | SubscriptionPtyInterestSet
                | SessionTranscriptOverlayRelease
                | WorkhubCoordinationActFromTurn
                | WorkhubCoordinationSelectAndDelegate
        )
}
fn failure(code: Code, message: &str) -> OperationError {
    OperationError {
        code,
        message: message.into(),
    }
}
