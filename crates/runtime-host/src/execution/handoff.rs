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

use super::{Executions, Result, failure, internal, provider};
use maka_event_log::turns::InvocationState;
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::event::InvocationOutcome;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

mod prepare;

pub(crate) struct CooperativeRun {
    pub invocation: maka_runtime::event::Invocation,
    pub gate: maka_agent::HandoffGate,
    pub completed: CancellationToken,
}

impl Executions {
    /// Caller owns admission, so this set cannot gain another execution.
    pub(crate) fn cooperative_runs(&self) -> Option<Vec<CooperativeRun>> {
        self.active
            .lock()
            .unwrap()
            .values()
            .map(|run| {
                Some(CooperativeRun {
                    invocation: run.invocation.clone(),
                    gate: run.handoff.clone()?,
                    completed: run.completed.clone(),
                })
            })
            .collect()
    }
    /// Notifications coalesce; canonical openings remain the sole claim authority.
    pub(crate) fn request_handoff_recovery(&self) {
        self.handoff_wake.notify_one();
    }

    pub(crate) fn start_handoff_recovery(self: &Arc<Self>, host_epoch: String) {
        let executions = self.clone();
        self.workers.spawn(async move {
            let mut delay = Duration::from_millis(250);
            let mut pending = false;
            loop {
                tokio::select! {
                    biased;
                    _ = executions.shutdown.cancelled() => break,
                    _ = executions.handoff_wake.notified() => { delay = Duration::from_millis(250); }
                    _ = tokio::time::sleep(delay), if pending => {
                        delay = (delay * 2).min(Duration::from_secs(30));
                    }
                }
                if executions.shutdown.is_cancelled() { break; }
                match executions.recover_handoffs(&host_epoch).await {
                    Ok(remaining) => pending = remaining,
                    Err(error) => {
                        executions.begin_drain();
                        eprintln!("handoff recovery failed: {}", error.message);
                        break;
                    }
                }
                // Also restores ownership of accepted queues after a reversible
                // preparation fence stopped their previous physical workers.
                if let Err(error) = executions.recover_messages().await {
                    executions.begin_drain();
                    eprintln!("pending message recovery failed: {}", error.message);
                    break;
                }
            }
        });
        self.request_handoff_recovery();
    }

    async fn recover_handoffs(self: &Arc<Self>, host_epoch: &str) -> Result<bool> {
        let mut after = 0;
        let mut remaining = false;
        loop {
            let page = self.log.pending_handoffs(after).await.map_err(internal)?;
            if page.is_empty() {
                return Ok(remaining);
            }
            for pending in page {
                after = pending.sequence;
                if !self.accepting() {
                    return Ok(true);
                }
                // The retiring Host must never reclaim its own sealed workers.
                if pending.host_epoch == host_epoch
                    || self.has_active_session(&pending.invocation.session_id)
                {
                    continue;
                }
                let candidate = match self.prepare_handoff(&pending).await {
                    Ok(candidate) => candidate,
                    Err(error) if parked(&error) => {
                        remaining = true;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let _admission = self.lock_admission().await;
                if !self.accepting() {
                    return Ok(true);
                }
                if self.has_active_session(&pending.invocation.session_id) {
                    continue;
                }
                let owner = self
                    .log
                    .handoff_owner(&pending.invocation)
                    .await
                    .map_err(internal)?;
                if owner.invocation != candidate.source.invocation
                    || !matches!(owner.state, InvocationState::Ended {
                        outcome: InvocationOutcome::HandoffPaused { ref pause }, ..
                    } if *pause == candidate.pause)
                {
                    continue;
                }
                let session = self
                    .log
                    .get_session::<crate::session::SessionConfiguration>(
                        &pending.invocation.session_id,
                    )
                    .await
                    .map_err(internal)?;
                if !session.is_some_and(|session| {
                    !session.archived && candidate.owns_workspace(&session.configuration)
                }) {
                    remaining = true;
                    continue;
                }
                match candidate.start(self).await {
                    Ok(started) => remaining |= !started,
                    Err(error) if parked(&error) => remaining = true,
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

fn parked(error: &maka_protocol::OperationError) -> bool {
    matches!(
        error.code,
        Code::OperationUnavailable
            | Code::OperationConflict
            | Code::SessionArchived
            | Code::NotFound
    )
}
