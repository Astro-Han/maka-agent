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

use maka_event_log::turns::{InvocationState, TurnBoundary};
use maka_protocol::turn::{LiveTurn, RootExecutionKind, TurnSnapshot, TurnState};
use maka_runtime::event::InvocationOutcome;
use maka_runtime::input::InvocationInput;

pub(crate) struct RecordedTurn {
    pub snapshot: TurnSnapshot,
    pub fingerprint: Option<String>,
    pub skill_invocation: maka_runtime::skills::SkillInvocationResult,
}

pub(crate) fn project(boundary: TurnBoundary) -> RecordedTurn {
    let invocation = boundary.invocation;
    let live = LiveTurn {
        root_execution_kind: matches!(boundary.input, InvocationInput::ContextCompact { .. })
            .then_some(RootExecutionKind::ContextCompact),
        ..LiveTurn::default()
    };
    let state = match boundary.state {
        InvocationState::Admitted => TurnState::Admitted(live),
        InvocationState::Running => TurnState::Running(live),
        InvocationState::WaitingForUser => TurnState::WaitingForUser(live),
        InvocationState::Ended {
            event_id: terminal_event_id,
            outcome,
        } => match outcome {
            InvocationOutcome::HandoffPaused { .. } => TurnState::Running(live),
            InvocationOutcome::Completed => TurnState::Completed {
                terminal_event_id,
                context_compaction_outcome: None,
            },
            InvocationOutcome::ContextCompactFinished { outcome } => TurnState::Completed {
                terminal_event_id,
                context_compaction_outcome: Some((&outcome).into()),
            },
            InvocationOutcome::Failed { class, message } => TurnState::Failed {
                terminal_event_id,
                failure_class: class,
                failure_message: message.as_ref().map(|message| bounded_message(message)),
            },
            InvocationOutcome::Cancelled { source } => TurnState::Cancelled {
                terminal_event_id,
                abort_source: source,
            },
        },
    };
    let skill_invocation = match &boundary.input {
        InvocationInput::Message {
            skill_invocation, ..
        } => skill_invocation.as_deref().cloned().unwrap_or_default(),
        _ => Default::default(),
    };
    RecordedTurn {
        skill_invocation,
        snapshot: TurnSnapshot {
            session_id: invocation.session_id,
            turn_id: invocation.turn_id,
            run_id: invocation.run_id,
            state,
        },
        fingerprint: match boundary.input {
            InvocationInput::Message {
                request_fingerprint,
                ..
            } => request_fingerprint,
            InvocationInput::ContextCompact {
                request_fingerprint,
            }
            | InvocationInput::Continuation {
                request_fingerprint,
                ..
            } => Some(request_fingerprint),
            InvocationInput::Code { .. } => None,
        },
    }
}

fn bounded_message(message: &str) -> String {
    let mut end = message.len().min(256);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_owned()
}
