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

mod call;
mod chunks;
mod dispatch;
mod forms;
mod frames;
mod state;
pub use dispatch::{ServiceCall, ToolCall};
pub use forms::{FormFuture, FormHandler};

use crate::Registration;
pub use call::{AcceptedCall, PendingCall};
use maka_protocol::capability::decode_host_frame;
use maka_runtime::capability::{ClientFrame, HostFrame};
use state::{Active, Inner, Invocation, Stage, ToolInvocation};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{oneshot, watch},
    time::Instant,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum CallError {
    #[error("Client Capability provider is unavailable")]
    CapabilityLost,
    #[error("Client Capability provider has too many active invocations")]
    Overloaded,
    #[error("Client Capability call timed out before admission")]
    TimedOut,
    #[error("Client Capability call was cancelled before admission")]
    Cancelled,
    #[error("Client Capability provider rejected the call: {0}")]
    ProviderRejected(String),
    #[error("Client Capability provider failed: {0}")]
    ProviderFailed(String),
    #[error("Client Capability call outcome is unknown: {0}")]
    OutcomeUnknown(&'static str),
    #[error("Invalid Client Capability call: {0}")]
    Invalid(&'static str),
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid Client Capability invocation frame: {0}")]
pub struct FrameError(&'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub current: u64,
    pub total: u64,
}

/// Owns all accepted reverse-call lifetimes, independent of any waiting future.
/// Composition must call shutdown before releasing the Host's root authority.
pub struct Broker {
    inner: Arc<Inner>,
}

impl Default for Broker {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                active: Mutex::new(Active::default()),
                draining: CancellationToken::new(),
                tasks: TaskTracker::new(),
            }),
        }
    }
}

impl Broker {
    /// Authorizes a callback from an already admitted tool, including calls
    /// pinned to a publication that has since been replaced or withdrawn.
    pub fn admitted_tool(
        &self,
        connection: Uuid,
        session: &str,
        turn: &str,
        tool_call: &str,
        server: &str,
        tool_name: &str,
    ) -> bool {
        let state = self.inner.active.lock().unwrap_or_else(|e| e.into_inner());
        !self.inner.draining.is_cancelled()
            && state.calls.values().any(|call| {
                let endpoint = call.registration.endpoint();
                call.registration.connection_id() == connection
                    && call.stage.admitted()
                    && call.pending_terminal.is_none()
                    && !call.cancellation.is_cancelled()
                    && !endpoint.closed().is_cancelled()
                    && !endpoint.invocations().is_cancelled()
                    && call.tool.as_ref().is_some_and(|tool| {
                        tool.session_id == session
                            && tool.turn_id == turn
                            && tool.tool_call_id == tool_call
                            && tool.server_id == server
                            && tool.tool_name == tool_name
                    })
            })
    }

    fn prepare(
        &self,
        registration: Arc<Registration>,
        timeout: Duration,
        cancellation: CancellationToken,
        frame: impl FnOnce(String) -> HostFrame,
    ) -> Result<PendingCall, CallError> {
        if timeout.is_zero() {
            return Err(CallError::Invalid("timeout must be positive"));
        }
        let invocation_id = Uuid::new_v4().to_string();
        let frame = frame(invocation_id.clone());
        let tool = match &frame {
            HostFrame::Call {
                session_id,
                turn_id,
                tool_call_id,
                server_id,
                tool_name,
                ..
            } => Some(ToolInvocation {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                tool_call_id: tool_call_id.clone(),
                server_id: server_id.clone(),
                tool_name: tool_name.clone(),
            }),
            _ => None,
        };
        let encoded =
            serde_json::to_value(&frame).map_err(|_| CallError::Invalid("JSON encoding"))?;
        decode_host_frame(&encoded).map_err(|_| CallError::Invalid("outbound wire boundary"))?;
        let endpoint = registration.endpoint().clone();
        let provider = endpoint.invocations();
        let mut state = self.inner.active.lock().unwrap_or_else(|e| e.into_inner());
        if self.inner.draining.is_cancelled() || cancellation.is_cancelled() {
            return Err(CallError::Cancelled);
        }
        if endpoint.closed().is_cancelled() || provider.is_cancelled() {
            return Err(CallError::CapabilityLost);
        }
        if state
            .calls
            .values()
            .filter(|call| call.registration.connection_id() == registration.connection_id())
            .count()
            >= 8
        {
            return Err(CallError::Overloaded);
        }
        let (accepted_tx, accepted) = oneshot::channel();
        let (result_tx, result) = oneshot::channel();
        let deadline_at = Instant::now()
            .checked_add(timeout)
            .ok_or(CallError::Invalid("timeout overflow"))?;
        let (deadline, deadlines) = watch::channel(Some(deadline_at));
        let (progress_tx, progress) = watch::channel(None);
        state.calls.insert(
            invocation_id.clone(),
            Invocation {
                registration,
                tool,
                stage: Stage::Dispatched(accepted_tx),
                result: result_tx,
                timeout,
                deadline,
                progress: progress_tx,
                cancellation: cancellation.clone(),
                form_handler: None,
                pending_terminal: None,
            },
        );
        if endpoint.send(frame).is_err() {
            Inner::settle(
                &mut state,
                &invocation_id,
                Err(CallError::CapabilityLost),
                false,
            );
            return Err(CallError::CapabilityLost);
        }
        // Spawn while holding the same gate shutdown uses, so close+wait cannot
        // miss a newly installed lifetime.
        self.monitor(
            invocation_id.clone(),
            deadlines,
            provider.clone(),
            cancellation,
        );
        Ok(PendingCall::new(
            self.inner.clone(),
            invocation_id,
            accepted,
            result,
            progress,
            provider,
        ))
    }

    /// Frames must first pass the protocol decoder. A violation belongs to the
    /// sending connection; callers close that peer, never a referenced victim.
    pub fn accept(&self, connection: Uuid, frame: ClientFrame) -> Result<(), FrameError> {
        self.inner.accept(connection, frame)
    }

    pub async fn shutdown(&self) {
        self.inner.begin_drain();
        self.inner.tasks.wait().await;
    }

    fn monitor(
        &self,
        id: String,
        mut deadlines: watch::Receiver<Option<Instant>>,
        provider: CancellationToken,
        caller: CancellationToken,
    ) {
        let inner = Arc::downgrade(&self.inner);
        let draining = self.inner.draining.clone();
        self.inner.tasks.spawn(async move {
            loop {
                let deadline = *deadlines.borrow_and_update();
                let expiration = async {
                    match deadline {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    biased;
                    _ = provider.cancelled() => {
                        if let Some(inner) = inner.upgrade() { inner.stop(&id, CallError::CapabilityLost, false); }
                        break;
                    }
                    _ = caller.cancelled() => {
                        if let Some(inner) = inner.upgrade() { inner.stop(&id, CallError::Cancelled, true); }
                        break;
                    }
                    _ = draining.cancelled() => {
                        if let Some(inner) = inner.upgrade() { inner.stop(&id, CallError::Cancelled, true); }
                        break;
                    }
                    changed = deadlines.changed() => if changed.is_err() { break; },
                    _ = expiration => {
                        let Some(inner) = inner.upgrade() else { break; };
                        if inner.expire(&id, deadline) { break; }
                    }
                }
            }
        });
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.inner.begin_drain();
    }
}
