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
    context::ModelPurpose,
    event::{EventWrite, Fact, Invocation, RuntimeEvent},
    tool_call::ToolCallIdentity,
    tool_output::ToolSuccess,
};
use serde_json::json;

/// A real request surface is required for replay and archive proof validation.
pub(super) async fn tool(
    log: &EventLog,
    invocation: &Invocation,
    step: &str,
    text: String,
) -> EventWrite {
    let source = log
        .read_model_context(
            &invocation.session_id,
            Some(&invocation.run_id),
            100,
            262144,
        )
        .await
        .unwrap();
    let event = |fact| EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap();
    log.append(&event(Fact::ModelRequested {
        step_id: step.into(),
        model_id: "test".into(),
        purpose: ModelPurpose::Main,
        context: None,
        source_scope: source.source_evidence.scope,
        source_high_water: source.source_evidence.high_water,
        source_digest: source.source_evidence.digest,
        input_digest: "fixture".into(),
        route_identity: format!("sha256:{}", "a".repeat(64)),
        checkpoint_event_id: source.baseline.map(|baseline| baseline.event_id),
        effective_source_digest: Some(source.effective_source_digest),
    }))
    .await
    .unwrap();
    log.append(&event(Fact::ModelCompleted {
        step_id: step.into(),
        output: serde_json::from_value(json!({
            "parts":[{"kind":"tool_call","call":{
                "id":step,"name":"Read","input":{"path":"file"},"provider_executed":false
            }}],"finish_reason":"tool-calls","usage":{}
        }))
        .unwrap(),
    }))
    .await
    .unwrap();
    let operation = format!("{step}:{step}");
    log.append(&event(Fact::ToolDispatched {
        operation_id: operation.clone(),
        call: ToolCallIdentity::provider(step.into(), step.into()),
        name: "Read".into(),
        input: json!({"path":"file"}),
    }))
    .await
    .unwrap();
    let (result, _) = EventWrite::tool_success(
        format!("result-{step}"),
        std::time::SystemTime::now(),
        invocation.clone(),
        operation,
        ToolSuccess::from(json!(text)),
    )
    .unwrap();
    log.append(&result).await.unwrap();
    result
}
