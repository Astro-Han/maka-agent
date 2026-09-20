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
    event::{EventWrite, Fact, Invocation, RuntimeEvent},
    input::DeliveredMessage,
};
#[path = "transcript.rs"]
mod transcript;
pub use transcript::transcript;

pub async fn provider_completion(log: &EventLog, session: &str, event_id: &str) -> EventWrite {
    use maka_runtime::{
        event::LogScope,
        model::{ModelEvent, ModelFinishReason, ModelPart, ModelStep, ModelToolCall},
    };
    let prefix = log
        .scoped_prefix(LogScope::Session { id: session.into() }, 100, 1024 * 1024)
        .await
        .unwrap();
    log.append(&write(
        session,
        Fact::ModelRequested {
            step_id: "provider-step".into(),
            model_id: "test".into(),
            source_scope: prefix.scope,
            source_high_water: prefix.high_water,
            source_digest: prefix.digest,
            input_digest: "input".into(),
            route_identity: "route".into(),
            effective_source_digest: None,
            purpose: maka_runtime::context::ModelPurpose::Main,
            context: None,
            checkpoint_event_id: None,
        },
    ))
    .await
    .unwrap();
    let call = ModelToolCall {
        id: "provider-call".into(),
        name: "remote".into(),
        input: serde_json::json!({}),
        provider_options: None,
        provider_executed: true,
    };
    for event in [
        ModelEvent::ToolCall(call.clone()),
        ModelEvent::ProviderToolResult {
            id: call.id.clone(),
            name: call.name.clone(),
            output: serde_json::json!({"ok":true}),
            is_error: false,
            provider_options: None,
        },
    ] {
        log.append(&write(
            session,
            Fact::ModelObserved {
                step_id: "provider-step".into(),
                event,
            },
        ))
        .await
        .unwrap();
    }
    let mut event = RuntimeEvent::new(
        invocation(session),
        Fact::ModelCompleted {
            step_id: "provider-step".into(),
            output: ModelStep {
                parts: vec![
                    ModelPart::ToolCall { call: call.clone() },
                    ModelPart::ToolResult {
                        id: call.id,
                        name: call.name,
                        output: serde_json::json!({"ok":true}),
                        is_error: false,
                        provider_options: None,
                    },
                ],
                finish_reason: ModelFinishReason::Stop,
                usage: Default::default(),
                provider_options: None,
                response_id: None,
                model: None,
                timestamp: None,
            },
        },
    );
    event.id = event_id.into();
    EventWrite::plain(event).unwrap()
}

pub async fn assert_provider_identity(log: &EventLog) {
    use maka_runtime::tool_call::provider_result_id;
    let claimed = provider_result_id("provider-future", 1);
    log.append(&steering("b", &claimed)).await.unwrap();
    let completion = provider_completion(log, "b", "provider-future").await;
    assert!(
        log.append(&completion).await.is_err(),
        "provider result cannot claim an existing client identity"
    );
    let mut event = completion.event().clone();
    event.id = "provider-completed".into();
    log.append(&EventWrite::plain(event).unwrap())
        .await
        .unwrap();
    assert!(
        log.append(&steering("b", &provider_result_id("provider-completed", 1)))
            .await
            .is_err(),
        "client identity cannot claim an existing provider result"
    );
}

pub fn invocation(session: &str) -> Invocation {
    Invocation {
        session_id: session.into(),
        turn_id: format!("turn-{session}"),
        run_id: format!("run-{session}"),
        invocation_id: format!("invocation-{session}"),
    }
}
pub fn write(session: &str, fact: Fact) -> EventWrite {
    EventWrite::plain(RuntimeEvent::new(invocation(session), fact)).unwrap()
}
pub fn steering(session: &str, id: &str) -> EventWrite {
    let mut content = maka_runtime::input::MessageInput::from("same text");
    // Retain prepared model text and submitted display text as different projections.
    content.display_text = Some("visible correction".into());
    write(
        session,
        Fact::MessageSteered {
            message: Box::new(DeliveredMessage {
                message_id: id.into(),
                content,
                submitted_content_digest: format!("sha256:{}", "a".repeat(64)),
            }),
        },
    )
}
