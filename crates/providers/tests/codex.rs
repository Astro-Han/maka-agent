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

use futures_util::future::BoxFuture;
use maka_plugins::{
    composition::Scope,
    contributions::Catalog,
    fiber::Fiber,
    http,
    model::{Connect, Credentials, Error as ModelError, Socket, Transport},
    provider::{Binding, Connection, Context, Error, authentication::Credential},
};
use maka_providers::codex::Codex;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct Exchange {
    started: Notify,
    release: Notify,
    requests: Mutex<Vec<http::Request>>,
    fail: bool,
}
impl Transport for Exchange {
    fn identity(&self) -> u64 {
        1
    }
    fn request(&self, request: http::Request) -> BoxFuture<'_, Result<http::Response, ModelError>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                return Err(ModelError::Adapter("reply lost".into()));
            }
            Ok(http::Response {
                head: http::Head { status: 200, url: "https://auth.openai.com/oauth/token".into(), headers: vec![] },
                body: Arc::new(Body(Mutex::new(Some(serde_json::to_vec(&json!({
                    "access_token": "replacement-access", "refresh_token": "replacement-refresh", "expires_in": 3600,
                })).unwrap())))),
            })
        })
    }
    fn connect(&self, _: Connect) -> BoxFuture<'_, Result<Arc<dyn Socket>, ModelError>> {
        Box::pin(async { Err(ModelError::Adapter("unexpected WebSocket".into())) })
    }
}
struct Body(Mutex<Option<Vec<u8>>>);
impl http::Body for Body {
    fn next(&self) -> BoxFuture<'_, Result<Option<Vec<u8>>, http::Error>> {
        Box::pin(async { Ok(self.0.lock().unwrap().take()) })
    }
    fn cancel(&self) {}
    fn close(&self) -> BoxFuture<'_, Result<(), http::Error>> {
        Box::pin(async { Ok(()) })
    }
}
fn connection() -> Connection {
    Connection {
        id: "connection".into(),
        revision: 1,
        configuration: json!({"baseUrl":"https://chatgpt.com/backend-api/codex"}),
    }
}
fn credential() -> Credential {
    Credential {
        secret: json!({
            "access_token":"old-access","refresh_token":"old+refresh/= &value","expires_at":1,"id_token":null,
        }).to_string(),
        refresh_at: Some(0),
    }
}

#[tokio::test]
async fn sent_refresh_settles_after_cancellation_and_unknown_reply_is_never_retried() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        for fail in [false, true] {
            let catalog = Catalog::default();
            let owner = Fiber::new(
                "example.subscription",
                "example.subscription",
                Scope::Profile,
            )
            .unwrap();
            owner.begin_loading().unwrap();
            owner.ready().unwrap();
            owner.publish().unwrap();
            let registration = catalog
                .register(&owner.context(), Codex::default().stage().unwrap())
                .unwrap();
            let binding = Binding::new(
                "chatgpt".into(),
                catalog
                    .snapshot(&Scope::Profile)
                    .entries
                    .remove("chatgpt")
                    .unwrap(),
            )
            .unwrap();
            let transport = Arc::new(Exchange {
                started: Notify::new(),
                release: Notify::new(),
                requests: Mutex::new(vec![]),
                fail,
            });
            let cancellation = CancellationToken::new();
            let context = Context {
                transport: transport.clone(),
                cancellation: cancellation.clone(),
                interaction: None,
            };
            let running = binding.clone();
            let refresh =
                tokio::spawn(
                    async move { running.refresh(connection(), credential(), context).await },
                );
            transport.started.notified().await;
            cancellation.cancel();
            drop(registration);
            transport.release.notify_one();
            let result = refresh.await.unwrap();
            if fail {
                assert!(matches!(result, Err(Error::OutcomeUnknown)));
            } else {
                let replacement = result.unwrap();
                assert!(replacement.refresh_at.is_some_and(|at| at > 0));
                let secret: serde_json::Value = serde_json::from_str(&replacement.secret).unwrap();
                assert_eq!(secret["refresh_token"], "replacement-refresh");
                assert!(matches!(
                    binding
                        .authorize(connection(), Some(replacement.clone()), "session".into())
                        .await,
                    Err(Error::Unavailable)
                ));
                let _registration = catalog
                    .register(&owner.context(), Codex::default().stage().unwrap())
                    .unwrap();
                let restored = Binding::resolve(binding.identity(), &catalog).unwrap();
                let Credentials::RequestHeaders(headers) = restored
                    .authorize(connection(), Some(replacement), "session".into())
                    .await
                    .unwrap()
                else {
                    panic!("provider must prepare protocol-neutral credentials")
                };
                assert_eq!(headers["authorization"], "Bearer replacement-access");
                assert_eq!(headers["session_id"], "session");
            }
            let requests = transport.requests.lock().unwrap();
            assert_eq!(
                requests.len(),
                1,
                "uncertain rotating grants cannot be retried"
            );
            assert_eq!(requests[0].url, "https://auth.openai.com/oauth/token");
            let form: std::collections::HashMap<_, _> =
                url::form_urlencoded::parse(&requests[0].body)
                    .into_owned()
                    .collect();
            assert_eq!(form["grant_type"], "refresh_token");
            assert_eq!(form["refresh_token"], "old+refresh/= &value");
        }
    })
    .await
    .unwrap();
}
