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

use crate::support::invocation;
use maka_agent::RunInput;
use maka_event_log::EventLog;
use maka_model::{ProviderConfig, ProviderKind};
use maka_runtime::{
    event::{Fact, Invocation},
    tools::{ToolExecutor, ToolFuture},
};
use maka_tools::{
    ToolCatalog, ToolDefinition, ToolHandler, ToolMode, ToolNesting, ToolRegistration,
    ToolSemantics,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

pub(super) struct Effect {
    pub log: Arc<EventLog>,
    pub count: Arc<AtomicUsize>,
}
impl ToolExecutor for Effect {
    fn names(&self) -> Vec<String> {
        vec!["echo".into()]
    }
    fn invoke(&self, _name: String, input: Value, _cancel: CancellationToken) -> ToolFuture {
        let log = self.log.clone();
        let count = self.count.clone();
        Box::pin(async move {
            let prefix = log.prefix(100, 128 * 1024).await.unwrap();
            assert!(prefix.events.iter().any(|event| matches!(
                &event.event.fact,
                Fact::ModelRequested { model_id, .. } if model_id == "test"
            )));
            assert!(matches!(
                prefix.events.last().unwrap().event.fact,
                Fact::ToolDispatched { .. }
            ));
            assert!(
                prefix
                    .events
                    .iter()
                    .any(|event| matches!(event.event.fact, Fact::ModelCompleted { .. }))
            );
            count.fetch_add(1, Ordering::SeqCst);
            Ok(input)
        })
    }
}

pub(super) fn input(base: &str, suffix: &str, effect: Arc<Effect>) -> RunInput {
    RunInput {
        main_output_limit: None,
        context: None,
        invocation: Invocation {
            session_id: "session".into(),
            turn_id: format!("turn-{suffix}"),
            run_id: format!("run-{suffix}"),
            invocation_id: format!("invocation-{suffix}"),
        },
        request_fingerprint: None,
        provider: ProviderConfig {
            kind: ProviderKind::OpenaiChat,
            model: "test".into(),
            base_url: base.into(),
            auth: maka_model::ProviderAuth::ApiKey("do-not-persist-this-secret".into()),
            headers: BTreeMap::new(),
            network: Default::default(),
            body_overlay: None,
        },
        provider_options: json!({}),
        supports_vision: false,
        configuration: invocation::configuration(ToolMode::Direct),
        work: maka_agent::RunWork::Message {
            skill_invocation: Default::default(),
            source_messages: Vec::new(),
            message: format!("question {suffix}").into(),
            tools: ToolCatalog::new([ToolRegistration {
                definition: ToolDefinition {
                    name: "echo".into(),
                    description: "echo".into(),
                    input_schema: json!({"type":"object"}),
                },
                nesting: ToolNesting::Nestable,
                semantics: ToolSemantics::Parallel,
                handler: ToolHandler::Immediate(effect),
            }])
            .unwrap(),
            max_steps: 3,
        },
    }
}

fn source(
    id: &str,
    content: maka_runtime::input::MessageInput,
    disposition: maka_runtime::message::MessageDisposition,
) -> maka_runtime::message::RootSourceMessage {
    maka_runtime::message::RootSourceMessage {
        message: maka_runtime::input::DeliveredMessage {
            message_id: id.into(),
            content,
            submitted_content_digest: format!("sha256:{}", "a".repeat(64)),
        },
        submitted_placement: maka_runtime::message::Placement::CurrentTurn,
        disposition,
        skill_invocation: Default::default(),
        submitted_intent: None,
    }
}
pub(super) async fn admit_root(log: &EventLog, input: &mut RunInput) {
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let maka_agent::RunWork::Message {
        message,
        source_messages,
        ..
    } = &mut input.work
    else {
        unreachable!()
    };
    let source = source(
        "user-first",
        message.clone(),
        maka_runtime::message::MessageDisposition::TurnStarted,
    );
    source_messages.push(source.clone());
    log.admit_message(
        maka_event_log::message_admissions::PendingMessageAdmission {
            steering_invocation: None,
            required_tools: Default::default(),
            invocation: input.invocation.clone(),
            source,
            admitted_at: 1,
        },
    )
    .await
    .unwrap();
}
pub(super) async fn enqueue(log: &EventLog, invocation: Invocation) {
    log.admit_message(
        maka_event_log::message_admissions::PendingMessageAdmission {
            steering_invocation: None,
            required_tools: Default::default(),
            invocation,
            source: source(
                "steer-first",
                "A queued correction".into(),
                maka_runtime::message::MessageDisposition::Steering,
            ),
            admitted_at: 2,
        },
    )
    .await
    .unwrap();
}
