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

use super::{CallError, FormHandler, Progress, chunks::Chunks};
use crate::Registration;
use maka_runtime::capability::{AdmissionEvidence, CallResult, HostFrame};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{oneshot, watch},
    time::Instant,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(super) enum Stage {
    Dispatched(oneshot::Sender<Result<AdmissionEvidence, CallError>>),
    Accepted,
    Admitted,
    AwaitingForm { cancellation: CancellationToken },
    Receiving(Chunks),
}
impl Stage {
    pub(super) fn admitted(&self) -> bool {
        matches!(
            self,
            Self::Admitted | Self::Receiving(_) | Self::AwaitingForm { .. }
        )
    }
}
pub(super) struct Invocation {
    pub registration: Arc<Registration>,
    pub tool: Option<ToolInvocation>,
    pub stage: Stage,
    pub result: oneshot::Sender<Result<CallResult, CallError>>,
    pub timeout: Duration,
    pub deadline: watch::Sender<Option<Instant>>,
    pub progress: watch::Sender<Option<Progress>>,
    pub cancellation: CancellationToken,
    pub form_handler: Option<Arc<dyn FormHandler>>,
    pub pending_terminal: Option<(Result<CallResult, CallError>, bool)>,
}

pub(super) struct ToolInvocation {
    pub session_id: String,
    pub turn_id: String,
    pub tool_call_id: String,
    pub server_id: String,
    pub tool_name: String,
}
#[derive(Default)]
pub(super) struct Active {
    pub calls: HashMap<String, Invocation>,
    pub(super) retired: HashSet<String>,
    order: VecDeque<String>,
}
pub(super) struct Inner {
    pub active: Mutex<Active>,
    pub draining: CancellationToken,
    pub tasks: TaskTracker,
}
impl Inner {
    pub fn begin_drain(&self) {
        let _gate = self.active.lock().unwrap_or_else(|e| e.into_inner());
        self.draining.cancel();
        self.tasks.close();
    }
    pub fn settle(
        state: &mut Active,
        id: &str,
        outcome: Result<CallResult, CallError>,
        release: bool,
    ) {
        if let Some(invocation) = state.calls.get_mut(id)
            && let Stage::AwaitingForm { cancellation, .. } = &invocation.stage
        {
            if invocation.pending_terminal.is_none() {
                invocation.pending_terminal = Some((outcome, release));
                cancellation.cancel();
            }
            return;
        }
        let Some(invocation) = state.calls.remove(id) else {
            return;
        };
        state.retired.insert(id.into());
        state.order.push_back(id.into());
        if state.order.len() > 1024
            && let Some(old) = state.order.pop_front()
        {
            state.retired.remove(&old);
        }
        if release {
            let _ = invocation.registration.endpoint().send(HostFrame::Release {
                invocation_id: id.into(),
            });
        }
        if let Stage::Dispatched(accepted) = invocation.stage {
            let error = outcome
                .as_ref()
                .err()
                .cloned()
                .unwrap_or(CallError::Invalid("settled before acceptance"));
            let _ = accepted.send(Err(error));
        }
        let _ = invocation.result.send(outcome);
    }
    pub fn stop(&self, id: &str, reason: CallError, cancel: bool) {
        let mut state = self.active.lock().unwrap_or_else(|e| e.into_inner());
        Self::stop_locked(&mut state, id, reason, cancel);
    }
    pub(super) fn stop_locked(state: &mut Active, id: &str, reason: CallError, cancel: bool) {
        let Some(invocation) = state.calls.get(id) else {
            return;
        };
        if invocation.pending_terminal.is_some() {
            return;
        }
        let outcome = if invocation.stage.admitted() {
            CallError::OutcomeUnknown("call cancelled, expired, or provider lost after admission")
        } else {
            reason
        };
        if cancel {
            let _ = invocation.registration.endpoint().send(HostFrame::Cancel {
                invocation_id: id.into(),
            });
        }
        Self::settle(state, id, Err(outcome), cancel);
    }
    pub fn expire(&self, id: &str, deadline: Option<Instant>) -> bool {
        let mut state = self.active.lock().unwrap_or_else(|e| e.into_inner());
        let Some(invocation) = state.calls.get(id) else {
            return true;
        };
        if *invocation.deadline.borrow() != deadline {
            return false;
        }
        Self::stop_locked(&mut state, id, CallError::TimedOut, true);
        true
    }
    pub fn admit(&self, id: &str, handler: Option<Arc<dyn FormHandler>>) {
        let mut state = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if self.cancelled(&mut state, id) {
            return;
        }
        let Some(invocation) = state.calls.get_mut(id) else {
            return;
        };
        if !matches!(invocation.stage, Stage::Accepted) {
            return;
        }
        let endpoint = invocation.registration.endpoint();
        if !invocation.registration.available() {
            Self::settle(&mut state, id, Err(CallError::CapabilityLost), false);
            return;
        }
        let Some(deadline) = Instant::now().checked_add(invocation.timeout) else {
            Self::settle(
                &mut state,
                id,
                Err(CallError::Invalid("timeout overflow")),
                false,
            );
            return;
        };
        invocation.stage = Stage::Admitted;
        invocation.form_handler = handler;
        invocation.deadline.send_replace(Some(deadline));
        if endpoint
            .send(HostFrame::Admitted {
                invocation_id: id.into(),
            })
            .is_err()
        {
            Self::settle(
                &mut state,
                id,
                Err(CallError::OutcomeUnknown(
                    "admission could not be delivered",
                )),
                false,
            );
        }
    }

    // Tokens are the authority; the lifetime task is only their wakeup path.
    // Recheck under the transition gate so an unpolled cancellation cannot
    // authorize an effect or accept a late result after drain has begun.
    pub(super) fn cancelled(&self, state: &mut Active, id: &str) -> bool {
        let Some(invocation) = state.calls.get(id) else {
            return true;
        };
        if !invocation.registration.available() {
            Self::stop_locked(state, id, CallError::CapabilityLost, false);
            true
        } else if invocation.cancellation.is_cancelled() || self.draining.is_cancelled() {
            Self::stop_locked(state, id, CallError::Cancelled, true);
            true
        } else {
            false
        }
    }
}
