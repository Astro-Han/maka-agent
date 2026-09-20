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
    event::{EventWrite, Fact, Invocation, InvocationInput, LogScope, RuntimeEvent},
    execution::InvocationConfiguration,
    model::{ModelFinishReason, ModelPart, ModelStep, ModelToolCall},
};

pub fn invocation() -> Invocation {
    Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    }
}

pub async fn record_model_call(
    log: &EventLog,
    invocation: &Invocation,
    configuration: &InvocationConfiguration,
    call: &ModelToolCall,
) {
    let facts = [
        Fact::InvocationOpened {
            configuration: Some(Box::new(configuration.clone())),
            input: InvocationInput::Message {
                source_messages: Vec::new(),
                content: "test".into(),
                request_fingerprint: None,
            },
        },
        Fact::ModelRequested {
            effective_source_digest: None,
            step_id: "step".into(),
            model_id: "fixture".into(),
            source_scope: LogScope::Root,
            source_high_water: 0,
            source_digest: "".into(),
            input_digest: "".into(),
            route_identity: "".into(),
            checkpoint_event_id: None,
            purpose: maka_runtime::context::ModelPurpose::Main,
            context: None,
        },
        Fact::ModelCompleted {
            step_id: "step".into(),
            output: ModelStep {
                parts: vec![ModelPart::ToolCall { call: call.clone() }],
                finish_reason: ModelFinishReason::ToolCalls,
                usage: Default::default(),
                provider_options: None,
                response_id: None,
                model: None,
                timestamp: None,
            },
        },
    ];
    log.append_batch(
        &facts.map(|fact| EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap()),
    )
    .await
    .unwrap();
}

pub fn interaction(invocation_id: &str) -> maka_runtime::capability::ClientFrame {
    maka_runtime::capability::ClientFrame::InteractionRequest {
        invocation_id: invocation_id.into(),
        interaction_id: "form".into(),
        request: maka_protocol::capability::decode_form_input(&serde_json::json!({
            "message":"Continue?", "requester":{"name":"Client"},
            "fields":[{"name":"proceed","label":"Continue","kind":"boolean","required":true}]
        }))
        .unwrap(),
    }
}
