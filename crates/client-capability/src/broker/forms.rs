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
    CallError,
    state::{Active, Inner, Stage},
};
use maka_runtime::capability::{FormInput, FormResult, HostFrame};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub type FormFuture = Pin<Box<dyn Future<Output = Result<FormResult, CallError>> + Send>>;

/// Host-owned canonical Form publication and answer resolution.
/// Cancellation must withdraw the canonical request before the future completes.
pub trait FormHandler: Send + Sync {
    fn request(&self, input: FormInput, cancellation: CancellationToken) -> FormFuture;
}

impl Inner {
    pub(super) fn start_form(
        self: &Arc<Self>,
        state: &mut Active,
        id: &str,
        interaction_id: String,
        request: FormInput,
    ) {
        let invocation = state.calls.get_mut(id).expect("active invocation");
        let Some(handler) = invocation.form_handler.clone() else {
            Self::stop_locked(state, id, CallError::Cancelled, true);
            return;
        };
        if invocation.deadline.borrow().expect("admitted deadline") <= Instant::now() {
            Self::stop_locked(state, id, CallError::TimedOut, true);
            return;
        }
        let cancellation = CancellationToken::new();
        invocation.stage = Stage::AwaitingForm {
            cancellation: cancellation.clone(),
        };
        invocation.deadline.send_replace(None);
        let inner = self.clone();
        let id = id.to_owned();
        // This task retains the owner even when Broker and the result waiter drop.
        // Creation occurs under the shutdown gate, before TaskTracker closes.
        self.tasks.spawn(async move {
            let result = handler.request(request, cancellation).await;
            inner.complete_form(&id, interaction_id, result);
        });
    }

    fn complete_form(
        &self,
        id: &str,
        interaction_id: String,
        result: Result<FormResult, CallError>,
    ) {
        let mut state = self.active.lock().unwrap_or_else(|e| e.into_inner());
        self.cancelled(&mut state, id);
        let Some(invocation) = state.calls.get_mut(id) else {
            return;
        };
        let Stage::AwaitingForm { .. } = invocation.stage else {
            return;
        };
        invocation.stage = Stage::Admitted;
        if let Some((outcome, release)) = invocation.pending_terminal.take() {
            Self::settle(&mut state, id, outcome, release);
            return;
        }
        match result {
            Ok(result) => {
                // Enqueue and resume share the transition gate: the provider may
                // immediately return a result or request its next Form.
                if invocation
                    .registration
                    .endpoint()
                    .send(HostFrame::InteractionResult {
                        invocation_id: id.into(),
                        interaction_id,
                        result,
                    })
                    .is_err()
                {
                    Self::stop_locked(&mut state, id, CallError::CapabilityLost, false);
                } else {
                    invocation
                        .deadline
                        .send_replace(Instant::now().checked_add(invocation.timeout));
                }
            }
            Err(_) => Self::stop_locked(&mut state, id, CallError::Cancelled, true),
        }
    }
}
