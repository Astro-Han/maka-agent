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

use super::Chat;
use maka_client::RequestFailure;
use maka_protocol::turn::{TurnSnapshot, TurnState, TurnStopInput};

/// A rendered action binds the exact run and observation generation, not a future selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub root: String,
    pub epoch: String,
    pub session: String,
    pub turn: String,
    pub run: String,
    generation: u64,
}
impl Target {
    pub fn input(&self) -> TurnStopInput {
        TurnStopInput {
            session_id: self.session.clone(),
            turn_id: self.turn.clone(),
            run_id: self.run.clone(),
        }
    }
}
#[derive(Default)]
pub struct Stop {
    target: Option<Target>,
    // A stop acknowledgement can still be Running: only observation confirms the end.
    pending: bool,
    error: Option<(&'static str, String)>,
}
impl Stop {
    pub fn pending(&self, target: &Target) -> bool {
        self.target.as_ref() == Some(target) && self.pending
    }
    pub fn status(&self, target: &Target) -> Option<(&'static str, &str)> {
        if self.target.as_ref() != Some(target) {
            return None;
        }
        if self.pending {
            Some(("chat-stopping", ""))
        } else {
            self.error.as_ref().map(|(key, text)| (*key, text.as_str()))
        }
    }
}
impl Chat {
    pub fn stop_target(&self, root: &str, epoch: &str) -> Option<Target> {
        if self.error.is_some() {
            return None;
        }
        let session = self.session.as_ref()?;
        let turn = self.snapshot.as_ref()?.root_turn.as_ref()?;
        if &turn.session_id != session
            || !matches!(
                turn.state,
                TurnState::Admitted(_)
                    | TurnState::Created(_)
                    | TurnState::Running(_)
                    | TurnState::WaitingForUser(_)
            )
        {
            return None;
        }
        Some(Target {
            root: root.into(),
            epoch: epoch.into(),
            session: session.clone(),
            turn: turn.turn_id.clone(),
            run: turn.run_id.clone(),
            generation: self.generation,
        })
    }
    pub fn start_stop(&mut self, target: &Target) -> bool {
        if self.stop_target(&target.root, &target.epoch).as_ref() != Some(target)
            || self.stop.pending(target)
        {
            return false;
        }
        self.stop = Stop {
            target: Some(target.clone()),
            pending: true,
            error: None,
        };
        true
    }
    pub fn stopped(&mut self, target: Target, result: Result<TurnSnapshot, RequestFailure>) {
        if self.generation != target.generation || self.stop.target.as_ref() != Some(&target) {
            return;
        }
        if let Err(error) = result {
            self.stop.pending = false;
            let key = match &error {
                RequestFailure::Unknown(_) => "chat-stop-unknown",
                _ => "chat-stop-failed",
            };
            self.stop.error = Some((key, error.to_string()));
        }
        // Success is cancellation accepted, not necessarily a terminal state.
        // Keep the control disabled until the canonical projection ends this run.
    }
}
