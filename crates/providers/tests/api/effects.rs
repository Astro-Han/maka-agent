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
use futures_util::future::BoxFuture;
use maka_plugins::{
    http,
    model::{Connect, Error as ModelError, Socket, Transport},
    provider::{Context, Discovery, Verification},
};
use maka_runtime::provider::Credential;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Exchange(Mutex<Vec<http::Request>>);
impl Transport for Exchange {
    fn identity(&self) -> u64 {
        1
    }
    fn request(&self, request: http::Request) -> BoxFuture<'_, Result<http::Response, ModelError>> {
        Box::pin(async move {
            let url = request.url.clone();
            let read_allowed = matches!(request.method, http::Method::Get);
            self.0.lock().unwrap().push(request);
            Ok(http::Response {
                head: http::Head {
                    status: 200,
                    url,
                    headers: vec![],
                },
                body: Arc::new(Body {
                    read_allowed,
                    bytes: Mutex::new(Some(
                        serde_json::to_vec(&json!({"data":[
                            {"id":"gpt-5.2"}, {"id":"claude-sonnet-future"}
                        ]}))
                        .unwrap(),
                    )),
                }),
            })
        })
    }
    fn connect(&self, _: Connect) -> BoxFuture<'_, Result<Arc<dyn Socket>, ModelError>> {
        Box::pin(async { Err(ModelError::Adapter("unexpected WebSocket".into())) })
    }
}
struct Body {
    bytes: Mutex<Option<Vec<u8>>>,
    read_allowed: bool,
}
impl http::Body for Body {
    fn next(&self) -> BoxFuture<'_, Result<Option<Vec<u8>>, http::Error>> {
        Box::pin(async {
            assert!(
                self.read_allowed,
                "verification must settle from status without waiting for a body"
            );
            Ok(self.bytes.lock().unwrap().take())
        })
    }
    fn cancel(&self) {}
    fn close(&self) -> BoxFuture<'_, Result<(), http::Error>> {
        Box::pin(async { Ok(()) })
    }
}
fn credential() -> Option<Credential> {
    Some(Credential {
        secret: "test-key".into(),
        refresh_at: None,
    })
}

#[tokio::test]
async fn verification_keeps_protocol_fields_and_rejects_conflicting_customization_before_io() {
    let (catalog, _owner, _registration) = registry();
    for (name, protocol, suffix, path) in [
        (
            "openai-compatible",
            ApiProtocol::OpenaiChat,
            "/v1/",
            "/v1/chat/completions",
        ),
        (
            "openai-responses-compatible",
            ApiProtocol::OpenaiResponses,
            "/v1/responses/",
            "/v1/responses",
        ),
        (
            "anthropic-compatible",
            ApiProtocol::AnthropicMessages,
            "/v1/",
            "/v1/messages",
        ),
    ] {
        let binding = binding(&catalog, name);
        let mut input = request(
            &binding,
            "future",
            ModelOverride {
                api_protocol: Some(protocol),
                ..Default::default()
            },
        );
        input.connection.configuration = json!({"baseUrl":format!("https://relay.test{suffix}")});
        let verification = || Verification {
            connection: input.connection.clone(),
            model: input.model.clone(),
            overrides: input.overrides.clone(),
            credential: credential(),
            request_headers: [("X-Route".into(), "private".into())].into(),
            request_body_overlay: Some(json!({"temperature":0.25})),
        };
        let transport = Arc::new(Exchange::default());
        let context = Context {
            transport: transport.clone(),
            cancellation: CancellationToken::new(),
            interaction: None,
        };
        binding
            .verify(verification(), context.clone())
            .await
            .unwrap();
        {
            let requests = transport.0.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].url, format!("https://relay.test{path}"));
            let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
            assert_eq!(body["model"], "future");
            assert_eq!(body["temperature"], 0.25);
            if protocol == ApiProtocol::OpenaiResponses {
                assert_eq!(body["store"], false);
                assert_eq!(body["max_output_tokens"], 16);
                assert!(body["input"].is_array());
            } else {
                assert_eq!(body["max_tokens"], 16);
                assert!(body["messages"].is_array());
            }
            let authentication = if protocol == ApiProtocol::AnthropicMessages {
                "x-api-key"
            } else {
                "authorization"
            };
            assert!(
                requests[0]
                    .headers
                    .iter()
                    .any(|(name, _)| name == authentication)
            );
        }
        let mut body_conflict = verification();
        body_conflict.request_body_overlay = Some(json!({"model":"different"}));
        assert!(
            binding
                .verify(body_conflict, context.clone())
                .await
                .is_err()
        );
        let mut header_conflict = verification();
        header_conflict.request_body_overlay = None;
        header_conflict
            .request_headers
            .insert("Content-Type".into(), "text/plain".into());
        assert!(binding.verify(header_conflict, context).await.is_err());
        assert_eq!(transport.0.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn discovery_survives_persistence_and_verification_does_not_require_streaming() {
    let (catalog, _owner, _registration) = registry();
    let transport = Arc::new(Exchange::default());
    let context = Context {
        transport: transport.clone(),
        cancellation: CancellationToken::new(),
        interaction: None,
    };
    for (name, id) in [("openai", "gpt-5.2"), ("anthropic", "claude-sonnet-future")] {
        let binding = binding(&catalog, name);
        let mut input = request(&binding, id, ModelOverride::default());
        let inventory = binding
            .discover(
                Discovery {
                    connection: input.connection.clone(),
                    credential: credential(),
                    request_headers: Default::default(),
                },
                context.clone(),
            )
            .await
            .unwrap();
        let persisted: Vec<ModelInfo> =
            serde_json::from_slice(&serde_json::to_vec(&inventory).unwrap()).unwrap();
        input.model = persisted.into_iter().find(|model| model.id == id).unwrap();
        let model = binding.prepare(input.clone()).await.unwrap();
        if name == "openai" {
            assert_eq!(
                model.provider_options["openai"]["reasoningEffort"],
                "medium"
            );
            assert_eq!(model.provider_options["openai"]["reasoningSummary"], "auto");
        } else {
            assert_eq!(model.info.capabilities.unwrap().vision, Some(true));
            input.model.capabilities.as_mut().unwrap().vision = Some(false);
            assert_eq!(
                binding
                    .prepare(input.clone())
                    .await
                    .unwrap()
                    .info
                    .capabilities
                    .unwrap()
                    .vision,
                Some(false)
            );
            input.model.capabilities.as_mut().unwrap().vision = Some(true);
            input.overrides.as_mut().unwrap().vision = Some(false);
            assert_eq!(
                binding
                    .prepare(input)
                    .await
                    .unwrap()
                    .info
                    .capabilities
                    .unwrap()
                    .vision,
                Some(false)
            );
        }
    }
    let anonymous = binding(&catalog, "localai");
    let input = request(&anonymous, "custom", ModelOverride::default());
    anonymous
        .discover(
            Discovery {
                connection: input.connection.clone(),
                credential: None,
                request_headers: Default::default(),
            },
            context.clone(),
        )
        .await
        .unwrap();
    anonymous
        .verify(
            Verification {
                connection: input.connection,
                model: input.model,
                overrides: None,
                credential: None,
                request_headers: Default::default(),
                request_body_overlay: None,
            },
            context.clone(),
        )
        .await
        .unwrap();
    {
        let requests = transport.0.lock().unwrap();
        for request in requests.iter().rev().take(2) {
            assert!(
                !request
                    .headers
                    .iter()
                    .any(|(name, _)| matches!(name.as_str(), "authorization" | "x-api-key")),
                "anonymous operations must not invent credentials"
            );
        }
    }
    let binding = binding(&catalog, "ollama-cloud");
    let input = request(&binding, "custom", ModelOverride::default());
    assert!(binding.prepare(input.clone()).await.is_err());
    binding
        .verify(
            Verification {
                connection: input.connection,
                model: input.model,
                overrides: None,
                credential: credential(),
                request_headers: Default::default(),
                request_body_overlay: None,
            },
            context,
        )
        .await
        .unwrap();
    let requests = transport.0.lock().unwrap();
    assert_eq!(
        requests.last().unwrap().url,
        "https://relay.test/v1/chat/completions"
    );
}
