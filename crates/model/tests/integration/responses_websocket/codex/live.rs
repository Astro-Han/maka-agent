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

use super::*;

/// Explicit opt-in. Read a nominated TS dev root without opening/migrating it;
/// no token refresh, credential copies, fixture writes or paid-model fallback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MAKA_CODEX_TEST_STATE_ROOT and authorized gpt-5.6-luna subscription access"]
async fn live_luna_subscription_streams_and_confirms_canonical_ws_continuation() {
    use std::io::Read;
    let root = std::path::PathBuf::from(
        std::env::var_os("MAKA_CODEX_TEST_STATE_ROOT").expect("explicit test root required"),
    );
    let read = |name: &str| -> Value {
        let mut bytes = Vec::new();
        std::fs::File::open(root.join(name))
            .unwrap()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        assert!(
            bytes.len() <= 4 * 1024 * 1024,
            "test root document exceeds limit"
        );
        serde_json::from_slice(&bytes).unwrap()
    };
    let catalog = read("connection-catalog.json");
    let vault = read("credential-vault.json");
    let policy = read("runtime-policy.json");
    let connection = catalog["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["providerType"] == "openai-codex"
                && row["enabled"] == true
                && row["enabledModelIds"]
                    .as_array()
                    .is_some_and(|models| models.contains(&json!("gpt-5.6-luna")))
        })
        .expect("enabled Luna subscription required");
    let entries = vault["entries"].as_array().unwrap();
    let entry = entries
        .iter()
        .find(|entry| {
            entry["locator"]["scope"] == "connection"
                && entry["locator"]["connectionId"] == connection["connectionId"]
                && entry["locator"]["kind"] == "oauth_token"
        })
        .expect("subscription credential required");
    let token: Value = serde_json::from_str(entry["secret"].as_str().unwrap()).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(
        token["expires_at"].as_u64().unwrap() > now + 60_000,
        "use the TS owner's normal refresh before this read-only probe"
    );
    let proxy: NetworkProxy =
        serde_json::from_value(policy["policy"]["networkProxy"].clone()).unwrap();
    let password = entries
        .iter()
        .find(|entry| entry["locator"]["scope"] == "network_proxy")
        .and_then(|entry| entry["secret"].as_str());
    let network = maka_network::Policy::from_settings(&proxy, password).unwrap();
    let provider = ProviderConfig {
        kind: ProviderKind::OpenaiResponses,
        model: "gpt-5.6-luna".into(),
        base_url: "https://chatgpt.com/backend-api/codex".into(),
        auth: ProviderAuth::Codex {
            access_token: token["access_token"].as_str().unwrap().to_owned(),
            session_id: format!("maka-rust-live-{now}"),
        },
        headers: BTreeMap::new(),
        body_overlay: None,
        network,
    };
    tokio::time::timeout(Duration::from_secs(140), async {
        let executor = ModelExecutor::new(1, Duration::from_secs(60)).unwrap();
        let lane = ResponsesLane::default();
        let mut prompt = vec![maka_model::prompt::Message::System {
            content: "Follow the user request concisely.".into(),
            provider_options: None,
        }];
        for (index, user) in [
            "Remember the token amber-120. Reply only with READY-120.",
            "What token did I ask you to remember? Reply only with the token.",
        ]
        .into_iter()
        .enumerate()
        {
            prompt.push(maka_model::prompt::Message::user(user));
            let request = ModelRequest {
                provider: provider.clone(),
                prompt: prompt.clone(),
                tools: vec![],
                provider_options: json!({"openai":{"reasoningEffort":"low"}}),
                max_output_tokens: None,
            };
            let step = generate_step(&executor, &lane, request).await;
            let text: String = step
                .parts
                .iter()
                .filter_map(|part| match part {
                    ModelPart::Text {
                        text_kind: TextKind::Text,
                        text,
                        ..
                    } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                text.trim(),
                if index == 0 { "READY-120" } else { "amber-120" }
            );
            prompt.push(
                serde_json::from_value(
                    json!({"role":"assistant","content":accepted_content(&step)}),
                )
                .unwrap(),
            );
            assert!(
                lane.needs_confirmation(),
                "HTTP completion cannot prove live WebSocket acceptance"
            );
            assert!(lane.confirm(&prompt, &[], step.response_id.as_deref()));
        }
        drop(lane);
    })
    .await
    .unwrap();
}
