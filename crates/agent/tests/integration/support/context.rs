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

use super::http;
use super::invocation;
pub use http::read_request;
use maka_agent::{Engine, RunInput, RunWork};
use maka_event_log::EventLog;
use maka_js_runtime::{CellLimits, CodeExecutor};
use maka_model::{ModelExecutor, ProviderConfig, ProviderKind};
use maka_runtime::{
    event::Invocation,
    execution::{ModelBinding, ToolMode},
};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{io::AsyncWriteExt, net::TcpStream};

pub const SUMMARY: &str = "## Goal\nContinue the durable task.\n## Progress\n### Done\n- Saved the original work.\n## Next Steps\n1. Verify checkpoint replay.\n## Critical Context\n- summary-marker in src/event.rs.";
pub fn engine(log: Arc<EventLog>) -> Engine {
    Engine::new(
        log,
        ModelExecutor::new(1, Duration::from_secs(10)).unwrap(),
        CodeExecutor::new(1, CellLimits::default()).unwrap(),
    )
}
pub fn input(base: &str, id: &str, compact: bool) -> RunInput {
    let mut configuration = invocation::configuration(ToolMode::Direct);
    configuration.model = Some(ModelBinding {
        connection_id: "connection".into(),
        connection_slug: "fixture".into(),
        model: "test".into(),
    });
    RunInput {
        main_output_limit: None,
        context: None,
        invocation: Invocation {
            session_id: "session".into(),
            turn_id: format!("turn-{id}"),
            run_id: format!("run-{id}"),
            invocation_id: format!("invocation-{id}"),
        },
        request_fingerprint: Some(format!(
            "{}:{id}",
            if compact {
                "context.compact"
            } else {
                "turn.start"
            }
        )),
        provider: ProviderConfig {
            kind: ProviderKind::OpenaiChat,
            model: "test".into(),
            base_url: base.into(),
            auth: maka_model::ProviderAuth::ApiKey("fixture".into()),
            headers: BTreeMap::new(),
            network: Default::default(),
            body_overlay: None,
        },
        provider_options: json!({}),
        supports_vision: false,
        configuration,
        work: if compact {
            RunWork::ContextCompact
        } else {
            RunWork::Message {
                source_messages: Vec::new(),
                message: format!("question-{id}").into(),
                tools: Default::default(),
                max_steps: 1,
            }
        },
    }
}
pub async fn respond(socket: &mut TcpStream, text: &str, reason: &str) {
    let first = json!({"id":"reply","object":"chat.completion.chunk","created":1,"model":"test","choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]});
    let last = json!({"id":"reply","object":"chat.completion.chunk","created":1,"model":"test","choices":[{"index":0,"delta":{},"finish_reason":reason}],"usage":{"prompt_tokens":20,"completion_tokens":50,"total_tokens":70}});
    let body = format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n");
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
}
