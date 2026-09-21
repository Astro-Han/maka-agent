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

use maka_model::{
    ModelExecutor, ModelRequest, ProviderAuth, ProviderConfig, ProviderKind, ResponsesLane,
    StepBuilder,
};
use maka_runtime::{
    model::{ModelPart, ModelSource},
    tools::{ProviderTool, ToolDefinition},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit MAKA_CODEX_AUTH_FILE and authorized gpt-5.6-luna subscription access"]
async fn codex_search_preserves_real_provider_calls_and_citations() {
    use std::io::Read;
    let path = std::env::var_os("MAKA_CODEX_AUTH_FILE").expect("explicit auth path required");
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .unwrap()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() <= 1024 * 1024);
    let auth: Value = serde_json::from_slice(&bytes).expect("invalid auth document");
    let access_token = auth["tokens"]["access_token"]
        .as_str()
        .expect("ChatGPT subscription token required")
        .to_owned();
    let mut network = maka_network::Policy::default();
    if let Ok(port) = std::env::var("MAKA_LIVE_PROXY_PORT") {
        let mut proxy = maka_runtime::configuration::policy::RuntimePolicy::default().network_proxy;
        proxy.enabled = true;
        proxy.host = "127.0.0.1".into();
        proxy.port = port.parse().expect("invalid proxy port");
        network = maka_network::Policy::from_settings(&proxy, None).unwrap();
    }
    tokio::time::timeout(Duration::from_secs(120), async {
        let executor = ModelExecutor::new(1, Duration::from_secs(60)).unwrap();
        let lane = ResponsesLane::default();
        let request = ModelRequest {
            provider: ProviderConfig {
                capabilities: Default::default(),
                kind: ProviderKind::OpenaiResponses,
                model: "gpt-5.6-luna".into(),
                base_url: "https://chatgpt.com/backend-api/codex".into(),
                auth: ProviderAuth::Codex {access_token, session_id: format!("maka-web-test-{}", std::process::id())},
                headers: BTreeMap::new(), body_overlay: None, network,
            },
            prompt: vec![
                maka_model::prompt::Message::System { content: "Use the provided web search tool for current information. Reply briefly with a cited source.".into(), provider_options: None },
                maka_model::prompt::Message::user("Search rust-lang.org for the current stable Rust release. Give its version and release date with a citation.")
            ],
            tools: vec![ToolDefinition {
                name: "WebSearch".into(),
                description: "Search the web".into(),
                input_schema: json!({"type":"object","properties":{}}),
                provider: Some(ProviderTool { id: "openai.web_search".into(), args: json!({"searchContextSize":"medium"}) }),
            }],
            provider_options: json!({"openai":{"reasoningEffort":"low"}}),
            max_output_tokens: None,
        };
        let mut stream = executor.stream_in_lane(request, CancellationToken::new(), Some(lane.clone())).await.unwrap();
        let mut builder = StepBuilder::for_step("live-native-search").unwrap();
        while let Some(event) = stream.next().await { builder.push(event.unwrap()).unwrap(); }
        let step = builder.finish().unwrap();
        stream.cancel_and_wait().await;
        let calls = step.tool_calls().filter(|call| call.provider_executed && call.name == "WebSearch").count();
        let sources = step.parts.iter().filter(|part| matches!(part, ModelPart::Source { source: ModelSource::Url { .. } })).count();
        println!("Codex native search: {calls} provider calls, {sources} cited sources; websocket={}", lane.needs_confirmation());
        assert!(calls > 0, "ordinary text is not evidence of a provider search");
        assert!(sources > 0, "citations must survive the SDK and canonical step");
        drop(lane);
    }).await.expect("live search must remain bounded");
}
