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

use maka_runtime::handoff::{HandoffIntent, HandoffPause};
use std::sync::Arc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
enum State {
    Running,
    Requested(Arc<HandoffIntent>),
    Held {
        pause: HandoffPause,
        cancellation: CancellationToken,
        ticket: Arc<HandoffIntent>,
    },
    Committed(HandoffPause),
    Sealed(HandoffPause),
    Closed,
}

/// Scheduling only. No gate state grants durable continuation authority.
#[derive(Clone)]
pub struct HandoffGate {
    state: watch::Sender<State>,
    source: Arc<maka_runtime::event::Invocation>,
    root_run_id: String,
}

/// Dropping preparation releases the same Run, without a terminal fact.
pub struct HandoffReservation {
    gate: HandoffGate,
    ticket: Arc<HandoffIntent>,
}

/// Only a currently held, fully settled step can request a durable seal.
pub struct HeldHandoff {
    reservation: HandoffReservation,
    pause: HandoffPause,
}

pub struct PendingSeal {
    gate: HandoffGate,
    pause: HandoffPause,
}

impl HandoffGate {
    pub fn root_run_id(&self) -> &str {
        &self.root_run_id
    }

    pub(crate) fn new(source: maka_runtime::event::Invocation, root_run_id: String) -> Self {
        Self {
            state: watch::channel(State::Running).0,
            source: Arc::new(source),
            root_run_id,
        }
    }

    pub fn reserve(&self, intent: HandoffIntent) -> Result<HandoffReservation, crate::RunError> {
        intent
            .validate(&self.source)
            .map_err(|e| crate::RunError::InvalidInput(e.into()))?;
        if intent.root_run_id != self.root_run_id {
            return Err(crate::RunError::InvalidInput(
                "handoff changes the logical Run".into(),
            ));
        }
        let ticket = Arc::new(intent);
        self.state
            .send_if_modified(|state| {
                if !matches!(state, State::Running) {
                    return false;
                }
                *state = State::Requested(ticket.clone());
                true
            })
            .then(|| HandoffReservation {
                gate: self.clone(),
                ticket,
            })
            .ok_or(crate::RunError::Busy)
    }

    pub(crate) async fn boundary<F: std::future::Future<Output = Option<HandoffPause>>>(
        &self,
        cancellation: &CancellationToken,
        preflight: impl FnOnce(HandoffIntent) -> F,
    ) -> Option<HandoffPause> {
        let mut changes = self.state.subscribe();
        let ticket = match &*changes.borrow_and_update() {
            State::Requested(ticket) => ticket.clone(),
            _ => return None,
        };
        let pause = preflight(ticket.as_ref().clone()).await;
        let Some(pause) =
            pause.filter(|pause| pause.intent == *ticket && !cancellation.is_cancelled())
        else {
            self.cancel(&ticket);
            return None;
        };
        self.state.send_if_modified(|state| {
            let State::Requested(intent) = state else {
                return false;
            };
            if !Arc::ptr_eq(intent, &ticket) {
                return false;
            }
            *state = State::Held {
                pause,
                cancellation: cancellation.clone(),
                ticket: intent.clone(),
            };
            true
        });
        loop {
            let state = changes.borrow_and_update().clone();
            match state {
                State::Committed(pause) if pause.intent == *ticket => return Some(pause),
                State::Held {
                    ticket: current, ..
                } if Arc::ptr_eq(&current, &ticket) => {}
                _ => return None,
            }
            tokio::select! {
                _ = changes.changed() => {},
                _ = cancellation.cancelled() => self.cancel(&ticket),
            }
        }
    }

    fn cancel(&self, ticket: &Arc<HandoffIntent>) {
        self.state.send_if_modified(|state| {
            let matches = match state {
                State::Requested(current)
                | State::Held {
                    ticket: current, ..
                } => Arc::ptr_eq(current, ticket),
                _ => false,
            };
            if matches {
                *state = State::Running;
            }
            matches
        });
    }

    /// Called only after the canonical pause append is acknowledged.
    pub(crate) fn sealed(&self, pause: &HandoffPause) {
        self.state.send_if_modified(|state| {
            if !matches!(state, State::Committed(expected) if expected == pause) {
                return false;
            }
            *state = State::Sealed(pause.clone());
            true
        });
    }

    pub(crate) fn close(&self) {
        self.state.send_if_modified(|state| {
            if matches!(state, State::Sealed(_) | State::Closed) {
                return false;
            }
            *state = State::Closed;
            true
        });
    }
}

impl HandoffReservation {
    pub async fn ready(self) -> Option<HeldHandoff> {
        let mut changes = self.gate.state.subscribe();
        loop {
            let state = changes.borrow_and_update().clone();
            match state {
                State::Held { pause, ticket, .. } if Arc::ptr_eq(&ticket, &self.ticket) => {
                    return Some(HeldHandoff {
                        reservation: self,
                        pause,
                    });
                }
                State::Requested(ticket) if Arc::ptr_eq(&ticket, &self.ticket) => {}
                _ => return None,
            }
            changes.changed().await.ok()?;
        }
    }
}

impl Drop for HandoffReservation {
    fn drop(&mut self) {
        self.gate.cancel(&self.ticket);
    }
}

impl HeldHandoff {
    pub fn preview(&self) -> &HandoffPause {
        &self.pause
    }

    /// Irreversible scheduling decision, not yet a durable receipt.
    pub fn commit(self) -> Option<PendingSeal> {
        let gate = &self.reservation.gate;
        gate.state
            .send_if_modified(|state| {
            if !matches!(state, State::Held { ticket, cancellation, .. } if Arc::ptr_eq(ticket, &self.reservation.ticket) && !cancellation.is_cancelled()) {
                    return false;
                }
                *state = State::Committed(self.pause.clone());
                true
            })
            .then(|| PendingSeal {
                gate: gate.clone(),
                pause: self.pause.clone(),
            })
    }
}

impl PendingSeal {
    /// None means the worker exited without confirming the requested seal.
    /// The owner must reconcile canonical facts, never resume the old worker.
    pub async fn wait(self) -> Option<HandoffPause> {
        let mut changes = self.gate.state.subscribe();
        loop {
            let state = changes.borrow_and_update().clone();
            match state {
                State::Sealed(pause) if pause == self.pause => return Some(pause),
                State::Committed(pause) if pause == self.pause => {}
                _ => return None,
            }
            changes.changed().await.ok()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(intent: HandoffIntent) -> std::future::Ready<Option<HandoffPause>> {
        use maka_runtime::handoff::{HandoffExecution, HandoffTools};
        std::future::ready(Some(HandoffPause {
            intent,
            remaining_steps: std::num::NonZeroU16::new(2).unwrap(),
            execution: Box::new(HandoffExecution {
                replay: maka_runtime::continuation::ReplayEvidence {
                    version: maka_runtime::continuation::REPLAY_VERSION,
                    digest: format!("sha256:{}", "a".repeat(64)),
                    route_identity: format!("sha256:{}", "b".repeat(64)),
                },
                context: None,
                provider_options: serde_json::json!({}),
                main_output_limit: None,
                supports_vision: false,
                tools: HandoffTools {
                    catalog_digest: format!("sha256:{}", "c".repeat(64)),
                    loaded: Default::default(),
                },
                compaction: maka_runtime::handoff::CompactionBudget::Available,
                replay_base: None,
            }),
        }))
    }

    #[tokio::test]
    async fn cancellation_prevents_commit_and_worker_exit_never_forges_a_seal() {
        let source = maka_runtime::event::Invocation {
            session_id: "session".into(),
            turn_id: "turn".into(),
            run_id: "run".into(),
            invocation_id: "invocation".into(),
        };
        let intent = HandoffIntent {
            handoff_id: "handoff".into(),
            host_epoch: "host".into(),
            root_run_id: source.run_id.clone(),
            successor_run_id: "successor-run".into(),
            successor_invocation_id: "successor-invocation".into(),
            claim_id: "claim".into(),
        };
        let gate = HandoffGate::new(source, "run".into());
        let cancellation = CancellationToken::new();
        let reservation = gate.reserve(intent.clone()).unwrap();
        let (result, held) = tokio::join!(
            gate.boundary(&cancellation, |_| { std::future::ready(None) }),
            reservation.ready(),
        );
        assert!(
            result.is_none() && held.is_none(),
            "failed preflight releases the same Run"
        );
        let reservation = gate.reserve(intent.clone()).unwrap();
        let (result, ()) = tokio::join!(gate.boundary(&cancellation, prepared), async {
            let held = reservation.ready().await.unwrap();
            cancellation.cancel();
            assert!(held.commit().is_none());
        },);
        assert!(result.is_none());

        let cancellation = CancellationToken::new();
        let reservation = gate.reserve(intent.clone()).unwrap();
        let (result, stale) = tokio::join!(gate.boundary(&cancellation, prepared), async {
            let held = reservation.ready().await.unwrap();
            cancellation.cancel();
            held
        },);
        assert!(result.is_none());
        let cancellation = CancellationToken::new();
        let reservation = gate.reserve(intent).unwrap();
        drop(stale); // The same durable intent does not reuse a scheduling owner.
        let (result, pending) = tokio::join!(gate.boundary(&cancellation, prepared), async {
            reservation.ready().await.unwrap().commit().unwrap()
        },);
        assert!(result.is_some());
        gate.close();
        assert!(
            pending.wait().await.is_none(),
            "commit is not an acknowledged append"
        );
        assert!(gate.reserve(result.unwrap().intent).is_err());
    }
}
