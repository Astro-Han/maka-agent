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

use maka_model::prompt::Message;
use maka_runtime::event::LogPrefix;

use crate::RunError;
mod chat;
mod compatible;
mod images;
pub(super) use chat::project as project_chat;
pub(super) use compatible::project as project_compatible;
mod output;
mod projection;
mod user;

#[derive(Clone, Copy)]
pub(super) enum EventRef<'a> {
    Canonical(&'a maka_runtime::event::StoredEvent),
    Archived(&'a maka_event_log::context::ArchivedToolResult),
}
impl<'a> EventRef<'a> {
    fn canonical(self) -> Option<&'a maka_runtime::event::StoredEvent> {
        match self {
            Self::Canonical(event) => Some(event),
            Self::Archived(_) => None,
        }
    }
}
impl<'a> From<&'a maka_event_log::context::ContextEvent> for EventRef<'a> {
    fn from(event: &'a maka_event_log::context::ContextEvent) -> Self {
        match event {
            maka_event_log::context::ContextEvent::Canonical(event) => Self::Canonical(event),
            maka_event_log::context::ContextEvent::Archived(event) => Self::Archived(event),
        }
    }
}
mod references;
pub(super) use images::materialize;
use projection::build;

pub fn operation_id(step_id: &str, call_id: &str) -> String {
    format!("{step_id}:{call_id}")
}

/// Only committed semantic facts produce provider input. No UI transcript or
/// previous isolate state is read, including after process restart.
pub fn project(prefix: &LogPrefix, session: &str) -> Result<Vec<Message>, RunError> {
    if prefix.events.iter().any(|stored| {
        matches!(
            stored.event.fact,
            maka_runtime::event::Fact::ToolResultArchived { .. }
        )
    }) {
        return Err(RunError::ReconciliationRequired(
            "archived history requires a checked effective context source".into(),
        ));
    }
    build(
        prefix.events.iter().map(EventRef::Canonical),
        session,
        &mut Vec::new(),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_runtime::event::Fact;
    use maka_runtime::event::{Invocation, LogScope, RuntimeEvent, StoredEvent};
    use maka_runtime::input::InvocationInput;
    use maka_runtime::tool_call::{ToolCallIdentity, ToolOrigin, ToolRejection};
    use serde_json::json;
    #[test]
    fn rejection_closes_only_its_proven_provider_call_as_error_text() {
        let invocation: Invocation = serde_json::from_value(json!({
            "session_id":"session","turn_id":"turn","run_id":"run","invocation_id":"invocation"
        }))
        .unwrap();
        let provider =
            |step: &str, call: &str| ToolCallIdentity::provider(step.into(), call.into());
        let rejected = |operation: &str, call| Fact::ToolRejected {
            operation_id: operation.into(),
            call,
            name: "exec".into(),
            input: json!({"code":"return 1"}),
            reason: ToolRejection::ExclusiveConflict,
        };
        let facts = [
            Fact::InvocationOpened {
                configuration: None,
                input: InvocationInput::Message {
                    skill_invocation: Default::default(),
                    source_messages: Vec::new(),
                    content: "hello".into(),
                    request_fingerprint: None,
                },
            },
            Fact::ModelRequested {
                effective_source_digest: None,
                purpose: None,
                context: None,
                checkpoint_event_id: None,
                step_id: "step".into(),
                model_id: "test".into(),
                source_scope: LogScope::Root,
                source_high_water: 1,
                source_digest: "fixture".into(),
                input_digest: "fixture".into(),
                route_identity: "fixture".into(),
            },
            Fact::ModelCompleted {
                step_id: "step".into(),
                output: serde_json::from_value(json!({
                    "parts":[{"kind":"tool_call","call":{
                        "id":"call","name":"exec","input":{"code":"return 1"},
                        "provider_executed":false
                    }}],
                    "finish_reason":"tool-calls","usage":{}
                }))
                .unwrap(),
            },
            rejected("hidden", ToolCallIdentity::standalone("hidden".into())),
            rejected(
                "nested",
                ToolCallIdentity {
                    tool_call_id: "nested".into(),
                    origin: ToolOrigin::CodeMode {
                        parent_operation_id: "parent".into(),
                        parent_tool_call_id: "parent-call".into(),
                    },
                },
            ),
            rejected("step:call", provider("step", "call")),
        ];
        let mut prefix = LogPrefix {
            scope: LogScope::Root,
            high_water: facts.len() as u64,
            digest: "test".into(),
            events: facts
                .into_iter()
                .enumerate()
                .map(|(index, fact)| StoredEvent {
                    sequence: index as u64 + 1,
                    event: RuntimeEvent::new(invocation.clone(), fact),
                })
                .collect(),
        };
        let history = project(&prefix, "session").unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(
            serde_json::to_value(&history[2]).unwrap(),
            json!({"role":"tool","content":[{
                "type":"tool-result","toolCallId":"call","toolName":"exec",
                "output":{"type":"error-text","value":"tool conflicts with exclusive step execution"}
            }]})
        );
        for stored in &mut prefix.events {
            stored.event =
                serde_json::from_value(serde_json::to_value(&stored.event).unwrap()).unwrap();
        }
        assert_eq!(project(&prefix, "session").unwrap(), history);
        let valid = prefix.events.last().unwrap().event.fact.clone();
        for invalid in [
            rejected("step:call", ToolCallIdentity::standalone("call".into())),
            rejected("step:call", provider("other", "call")),
            rejected("step:call", provider("step", "wrong-call")),
        ] {
            prefix.events.last_mut().unwrap().event.fact = invalid;
            assert!(project(&prefix, "session").is_err());
        }
        prefix.events.last_mut().unwrap().event.fact = valid;
        prefix.events.insert(
            4,
            StoredEvent {
                sequence: 5,
                event: RuntimeEvent::new(
                    invocation,
                    Fact::ToolDispatched {
                        operation_id: "step:call".into(),
                        call: provider("step", "call"),
                        name: "exec".into(),
                        input: json!({"code":"return 1"}),
                    },
                ),
            },
        );
        assert!(project(&prefix, "session").is_err());
    }
}
