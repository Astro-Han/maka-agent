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

use maka_event_log::EventLog;
use maka_runtime::{
    event::{EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent},
    execution::{BehaviorId, CollaborationMode, InvocationConfiguration, PermissionMode, ToolMode},
    executor::{Binding, Output},
    tool_call::ToolCallIdentity,
};
use serde_json::json;

#[tokio::test]
async fn external_output_is_durable_observation_not_native_dispatch_and_rebuilds_transcript() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("executors.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let invocation = Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    let event = |fact| EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap();
    log.append(&event(Fact::InvocationOpened {
        input: InvocationInput::Message {
            content: "external task".into(),
            source_messages: vec![],
            request_fingerprint: None,
        },
        configuration: Some(Box::new(InvocationConfiguration {
            cwd: temp.path().to_string_lossy().into_owned(),
            workspace_identity: None,
            model: None,
            tool_composition: None,
            system_prompt: None,
            thinking_level: None,
            permission_mode: PermissionMode::Ask,
            collaboration_mode: CollaborationMode::Agent,
            orchestration_mode: BehaviorId::default(),
            tool_mode: ToolMode::Direct,
        })),
    }))
    .await
    .unwrap();
    assert!(
        log.append(&event(Fact::ExecutorObserved {
            output: Output::OutputDelta {
                text: "too early".into()
            }
        }))
        .await
        .is_err()
    );
    let started = event(Fact::ExecutorStarted {
        binding: Binding {
            executor_id: "example".to_owned().try_into().unwrap(),
            package_id: "example".into(),
            entry_id: "example".into(),
            activation: "activation".into(),
        },
    });
    log.append(&started).await.unwrap();
    for output in [
        Output::ThinkingDelta {
            text: "external reasoning".into(),
        },
        Output::OutputDelta {
            text: "partial answer".into(),
        },
        Output::ToolStart {
            tool_call_id: "external-call".into(),
            name: "shell".into(),
            input: json!({"command":"external only"}),
        },
        Output::ToolProgress {
            tool_call_id: "external-call".into(),
            text: "working".into(),
        },
    ] {
        log.append(&event(Fact::ExecutorObserved { output }))
            .await
            .unwrap();
    }
    let before = log.prefix(100, 1024 * 1024).await.unwrap();
    let observation = log
        .observe_session::<serde_json::Value>("session")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observation.active_streams.len(), 2);
    assert!(
        observation
            .active_streams
            .iter()
            .all(|seed| seed.message_id == started.event().id)
    );
    let stream = log
        .session_stream_events("session", 0, before.high_water, 32, 1024 * 1024)
        .await
        .unwrap();
    use maka_event_log::observation::StreamFact;
    use maka_runtime::model::TextKind;
    assert!(stream.events.iter().any(|event| matches!(&event.fact,
        StreamFact::ExecutorDelta {text_kind: TextKind::Thinking, text} if text == "external reasoning")));
    assert!(stream.events.iter().any(|event| matches!(&event.fact,
        StreamFact::ExecutorToolProgress {text, ..} if text == "working")));
    let overlay = log
        .active_transcript(&invocation, before.high_water)
        .await
        .unwrap();
    assert_eq!(overlay.len(), 1);
    let overlay = serde_json::to_value(&overlay[0]).unwrap();
    assert_eq!(overlay["text"], "partial answer");
    assert_eq!(overlay["thinking"]["text"], "external reasoning");
    assert!(
        log.append(&event(Fact::ToolDispatched {
            operation_id: "dispatch".into(),
            call: ToolCallIdentity::standalone("call".into()),
            name: "shell".into(),
            input: json!({})
        }))
        .await
        .is_err()
    );
    assert!(
        log.append(&event(Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed
        }))
        .await
        .is_err()
    );
    log.append(&event(Fact::ExecutorObserved {
        output: Output::ToolResult {
            tool_call_id: "external-call".into(),
            text: "external result".into(),
            is_error: false,
        },
    }))
    .await
    .unwrap();
    log.append(&event(Fact::ExecutorCompleted {
        text: "final authoritative answer".into(),
    }))
    .await
    .unwrap();
    assert!(
        log.append(&event(Fact::ExecutorObserved {
            output: Output::OutputDelta {
                text: "too late".into()
            }
        }))
        .await
        .is_err()
    );
    let through = log
        .append(&event(Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        }))
        .await
        .unwrap();
    assert!(
        log.observe_session::<serde_json::Value>("session")
            .await
            .unwrap()
            .unwrap()
            .active_streams
            .is_empty()
    );
    assert!(
        log.prepare_transcript("session", through, 32)
            .await
            .unwrap()
    );
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert!(
        log.prepare_transcript("session", through, 32)
            .await
            .unwrap()
    );
    let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
    let mut view = maka_presentation::InvocationView::new(1024 * 1024).unwrap();
    let mut rows = vec![];
    for record in &prefix.events {
        rows.extend(view.push(record).unwrap());
    }
    let rows = rows
        .iter()
        .map(|row| serde_json::to_value(&row.message).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        rows.iter().filter(|row| row["type"] == "tool_call").count(),
        1
    );
    assert!(
        rows.iter()
            .any(|row| row["type"] == "tool_call" && row["providerExecuted"] == true)
    );
    assert!(rows.iter().any(|row| row["type"] == "assistant"
        && row["text"] == "final authoritative answer"
        && row["thinking"]["text"] == "external reasoning"));
    assert!(!prefix.events.iter().any(|row| matches!(
        row.event.fact,
        Fact::ToolDispatched { .. } | Fact::ModelRequested { .. }
    )));
    log.close().await.unwrap();
}
