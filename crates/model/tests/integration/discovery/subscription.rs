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
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use std::process::Stdio;

fn token(claims: Value) -> String {
    format!(
        "e30.{}.signature",
        URL_SAFE_NO_PAD.encode(claims.to_string())
    )
}

fn copilot(id: &str) -> Value {
    json!({"id":id,"model_picker_enabled":true,"supported_endpoints":["/responses"],
        "capabilities":{"supports":{"tool_calls":true}}})
}

#[tokio::test]
async fn account_probe_requires_selected_model_and_preserves_http_failure_status() {
    let mut blocked = copilot("chosen");
    blocked["policy"] = json!({"state":"disabled"});
    let mut anthropic = copilot("chosen");
    anthropic["supported_endpoints"] = json!(["/v1/messages"]);
    for (status, payload, failure) in [
        (200, json!({"data":[anthropic]}), None),
        (
            200,
            json!({"data":[copilot("other")]}),
            Some((Failure::Unknown, None)),
        ),
        (200, json!({"data":[]}), Some((Failure::Unknown, None))),
        (200, json!({"data":[blocked]}), Some((Failure::Auth, None))),
        (
            200,
            json!({"data":[false]}),
            Some((Failure::InvalidResponse, None)),
        ),
        (403, json!({}), Some((Failure::Auth, Some(403)))),
        (
            429,
            json!({}),
            Some((Failure::ProviderUnavailable, Some(429))),
        ),
    ] {
        let (base, server) = fixture(
            format!("HTTP/1.1 {status} Test\r\nConnection: close\r\n\r\n{payload}").into_bytes(),
        )
        .await;
        let policy = maka_network::Policy::from_settings(
            &maka_runtime::configuration::policy::NetworkProxy {
                enabled: true,
                protocol: maka_runtime::configuration::policy::ProxyProtocol::Http,
                host: "127.0.0.1".into(),
                port: reqwest::Url::parse(&base).unwrap().port().unwrap(),
                auth_enabled: true,
                username: "proxy-user".into(),
                bypass_list: vec![],
                auto_bypass_domains: vec![],
            },
            Some("proxy-password"),
        )
        .unwrap();
        let client = ConnectionClient::with_policy(&policy).unwrap();
        let result = client
            .test_inventory(
                DiscoveryRequest {
                    kind: DiscoveryKind::Copilot,
                    base_url: "http://fixture.invalid/v1",
                    credential: "gho_fixture",
                    headers: &BTreeMap::new(),
                },
                "chosen",
            )
            .await;
        match failure {
            None => {
                result.unwrap();
            }
            Some((class, status)) => {
                let error = result.unwrap_err();
                assert_eq!((error.class, error.status_code), (class, status));
            }
        }
        let request = server.await.unwrap();
        assert!(request.starts_with("GET http://fixture.invalid/v1/models HTTP/1.1\r\n"));
        assert!(request.contains("\r\nauthorization: Bearer gho_fixture\r\n"));
        assert!(request.contains("\r\nproxy-authorization: Basic "));
        assert!(request.contains("\r\ncopilot-integration-id: vscode-chat\r\n"));
    }
}

#[tokio::test]
async fn subscription_inventories_match_source_headers_account_routing_and_model_semantics() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut cases = Vec::new();
        let inventory = json!({"models":[
            {"slug":"hidden","visibility":" HIDDEN "},
            {"slug":" fallback ","priority":null,"context_window":0},
            {"slug":"second","priority":2,"context_window":4096.0},
            {"slug":" first ","priority":1,"context_window":8192},
            {"slug":"first","priority":1,"context_window":4096},
            {"slug":"tie","priority":1},{"slug":"","priority":0}
        ]});
        for claims in [
            json!({"chatgpt_account_id":"Direct","https://api.openai.com/auth":{"chatgpt_account_id":"Nested"}}),
            json!({"https://api.openai.com/auth":{"chatgpt_account_id":"Nested"}}),
            json!({"organizations":[{"id":" "},{"id":" Org "},{"id":"Other"}]}),
            json!({"sub":"MustNotBeAccount"}),
        ] {
            cases.push(json!({"provider":"openai-codex","token":token(claims),"payload":inventory}));
        }
        let mut rich = copilot("rich");
        rich["name"] = "Named model".into();
        rich["supported_endpoints"] = json!(["/chat/completions","/responses","/v1/messages"]);
        rich["capabilities"]["supports"]["reasoning_effort"] = json!(["low"]);
        rich["capabilities"]["limits"] = json!({"max_context_window_tokens":null,
            "max_prompt_tokens":8192.0,"max_output_tokens":1e3,"vision":{"supported_media_types":["image/png"]}});
        let mut blocked = copilot("blocked");
        blocked["policy"] = json!({"state":"unconfigured"});
        let mut hidden = copilot("not-picker");
        hidden["model_picker_enabled"] = false.into();
        let mut chat = copilot("chat");
        chat["supported_endpoints"] = json!(["/chat/completions"]);
        let mut no_tools = copilot("no-tools");
        no_tools["capabilities"]["supports"]["tool_calls"] = false.into();
        let mut unknown_policy = copilot("unknown-policy");
        unknown_policy["policy"] = json!({"state":"other"});
        let mut malformed = copilot("bad-array");
        malformed["capabilities"]["supports"]["reasoning_effort"] = Value::Null;
        for payload in [
            json!({"data":[rich,copilot("responses"),chat,blocked.clone(),hidden,no_tools,unknown_policy]}),
            json!({"data":[blocked]}),
            json!({"data":[copilot("valid"),malformed]}),
            json!({"data":[1]}), json!({}), json!({"data":[]}),
        ] {
            cases.push(json!({"provider":"github-copilot","token":"gho_Fixture","payload":payload}));
        }
        for payload in [json!({}),json!({"models":null}), json!({"models":[null]}),
            json!({"models":[{"slug":"bad-limit","context_window":0.5}]}), json!({"models":[]})] {
            cases.push(json!({"provider":"openai-codex","token":"opaque","payload":payload}));
        }
        cases.push(json!({"provider":"xai-oauth","token":"Xai-Fixture",
            "payload":{"data":[{"id":"grok","context_window":4096,"supports_reasoning":true}]}}));
        for provider in ["openai-codex","github-copilot","xai-oauth"] {
            for status in [401,403,429,503] {
                cases.push(json!({"provider":provider,"token":"synthetic","status":status,"payload":{}}));
            }
        }
        let expected = oracle(&cases).await;
        for (case, expected) in cases.iter().zip(expected.as_array().unwrap()) {
            let status = case["status"].as_u64().unwrap_or(200);
            let response = format!("HTTP/1.1 {status} Test\r\nConnection: close\r\n\r\n{}",case["payload"]);
            let (base, server) = fixture(response.into_bytes()).await;
            let address = reqwest::Url::parse(&base).unwrap();
            let policy = maka_network::Policy::from_settings(
                &maka_runtime::configuration::policy::NetworkProxy {
                    enabled: true,
                    protocol: maka_runtime::configuration::policy::ProxyProtocol::Http,
                    host: "127.0.0.1".into(), port: address.port().unwrap(),
                    auth_enabled: true, username: "user".into(),
                    bypass_list: vec![], auto_bypass_domains: vec![],
                }, Some("secret")
            ).unwrap();
            let client = ConnectionClient::with_policy(&policy).unwrap();
            let result = client.discover(DiscoveryRequest {
                kind: match case["provider"].as_str().unwrap() {
                    "openai-codex" => DiscoveryKind::Codex,
                    "github-copilot" => DiscoveryKind::Copilot,
                    _ => DiscoveryKind::Openai,
                },
                base_url: "http://fixture.invalid/v1/",
                credential: case["token"].as_str().unwrap(),
                headers: &BTreeMap::new(),
            }).await;
            let wire = server.await.unwrap();
            let reference = &expected["requests"][0];
            let path = reference["url"].as_str().unwrap();
            assert!(wire.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
            let headers: BTreeMap<_,_> = wire.lines().skip(1).filter_map(|line| line.split_once(':'))
                .map(|(name,value)| (name.to_ascii_lowercase(),value.trim())).collect();
            assert_eq!(headers.get("proxy-authorization"), Some(&"Basic dXNlcjpzZWNyZXQ="));
            for (name,value) in reference["headers"].as_object().unwrap() {
                assert_eq!(headers.get(name).copied(),value.as_str(),"{name}");
            }
            for generated in ["authorization","chatgpt-account-id","openai-beta","originator",
                "editor-version","editor-plugin-version","copilot-integration-id","openai-intent",
                "x-github-api-version","user-agent"] {
                assert_eq!(headers.contains_key(generated),reference["headers"].get(generated).is_some(),"{generated}");
            }
            let actual = match result {
                Ok(models) => json!({"ok":true,"models":models}),
                Err(kind) => json!({"ok":false,"error":{"kind":kind}}),
            };
            let mut projected = expected["result"].clone();
            // The public discovery outcome carries errorClass, not HTTP statusCode.
            if let Some(error) = projected.get_mut("error").and_then(Value::as_object_mut)
                && let Some(code) = error.remove("statusCode") {
                assert_eq!(code, status);
            }
            assert_eq!(actual,projected,"provider {} payload {}",case["provider"],case["payload"]);
        }
    }).await.unwrap();
}

async fn oracle(cases: &[Value]) -> Value {
    let mut child = tokio::process::Command::new("node")
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/subscription-discovery-oracle.mjs"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(cases).unwrap().as_bytes())
        .await
        .unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
