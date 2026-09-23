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

use crate::{decision::*, settings::*};
use futures_util::future::BoxFuture;
use maka_plugins::{
    call, composition::Scope, credentials, fiber::Fiber, http, preferences, services::Services,
    storage,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

struct Config(Settings);
impl storage::Store for Config {
    fn read(
        &self,
        _: String,
    ) -> BoxFuture<'_, Result<Option<storage::Record>, storage::StoreError>> {
        Box::pin(async {
            Ok(Some(storage::Record {
                revision: 1,
                data: storage::Data::Present(serde_json::to_value(&self.0).unwrap()),
            }))
        })
    }
    fn scan(&self, _: storage::Scan) -> BoxFuture<'_, Result<storage::Page, storage::StoreError>> {
        Box::pin(async { unreachable!() })
    }
    fn batch(
        &self,
        _: Vec<storage::Mutation>,
    ) -> BoxFuture<'_, Result<Vec<storage::Record>, storage::StoreError>> {
        Box::pin(async { unreachable!() })
    }
}
struct Keys {
    endpoint: String,
}
impl credentials::Credentials for Keys {
    fn read(
        &self,
        key: String,
    ) -> BoxFuture<'_, Result<Option<credentials::Record>, storage::StoreError>> {
        Box::pin(async move {
            Ok((key == self.endpoint).then(|| credentials::Record {
                revision: 1,
                secret: Some(
                    json!({"apiKey":"private-key","headers":{"X-Tenant":"tenant"}}).to_string(),
                ),
            }))
        })
    }
    fn write(
        &self,
        _: credentials::Write,
    ) -> BoxFuture<'_, Result<credentials::WriteResult, storage::StoreError>> {
        Box::pin(async { unreachable!() })
    }
}
#[derive(Default)]
struct Privacy(AtomicBool);
impl preferences::Preferences for Privacy {
    fn read(&self) -> BoxFuture<'_, Result<preferences::Snapshot, maka_plugins::Error>> {
        Box::pin(async {
            Ok(serde_json::from_value(json!({"revision":1,"privacy":{"incognitoActive":self.0.load(Ordering::SeqCst)},"personalization":{"displayName":"","assistantTone":""},"workspaceInstructions":false})).unwrap())
        })
    }
}
#[derive(Default)]
struct Transport {
    replies: Mutex<VecDeque<http::Response>>,
    calls: Mutex<Vec<(call::Scope, http::Request)>>,
    started: tokio::sync::Notify,
}
impl http::Client for Transport {
    fn request(
        &self,
        scope: call::Scope,
        request: http::Request,
    ) -> BoxFuture<'_, Result<http::Response, http::Error>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((scope, request));
            self.started.notify_one();
            let reply = self.replies.lock().unwrap().pop_front();
            match reply {
                Some(r) => Ok(r),
                None => std::future::pending().await,
            }
        })
    }
}
struct Body {
    bytes: Mutex<Option<Vec<u8>>>,
    closes: AtomicUsize,
}
impl http::Body for Body {
    fn next(&self) -> BoxFuture<'_, Result<Option<Vec<u8>>, http::Error>> {
        Box::pin(async { Ok(self.bytes.lock().unwrap().take()) })
    }
    fn cancel(&self) {}
    fn close(&self) -> BoxFuture<'_, Result<(), http::Error>> {
        Box::pin(async {
            self.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
fn reply(t: &Transport, status: u16, bytes: Vec<u8>) -> Arc<Body> {
    let body = Arc::new(Body {
        bytes: Mutex::new(Some(bytes)),
        closes: AtomicUsize::new(0),
    });
    t.replies.lock().unwrap().push_back(http::Response {
        head: http::Head {
            status,
            url: "https://custom.example/jev".into(),
            headers: vec![],
        },
        body: body.clone(),
    });
    body
}
fn input() -> Evaluation {
    serde_json::from_value(json!({"state":{"complete":true},"questions":{"done":{"type":"noul","instructions":"Is complete true?"}}})).unwrap()
}
fn answer() -> Vec<u8> {
    serde_json::to_vec(&json!({"model":"custom","answers":{"done":{"type":"noul","noul":0.8}},"usage":{"input_tokens":15,"output_tokens":2}})).unwrap()
}
async fn scope() -> call::Scope {
    call::Issuer::default()
        .admit(
            call::Identity::Remote {
                request_id: uuid::Uuid::new_v4(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap()
}
fn backend() -> (Jev, Arc<Transport>, Arc<Privacy>) {
    let settings = Settings {
        enabled: true,
        url: "https://custom.example/jev".into(),
        model: "custom".into(),
        timeout_ms: 100,
    };
    let transport = Arc::new(Transport::default());
    let privacy = Arc::new(Privacy::default());
    (
        Jev {
            settings: Repository {
                credentials: Arc::new(Keys {
                    endpoint: settings.credential_key(),
                }),
                store: Arc::new(Config(settings)),
            },
            http: transport.clone(),
            preferences: privacy.clone(),
        },
        transport,
        privacy,
    )
}
#[tokio::test]
async fn callers_share_typed_and_json_service_and_retirement_rejects_old_handle() {
    let (jev, t, _) = backend();
    let fiber = Fiber::new("maka.jev", "maka.jev", Scope::Profile).unwrap();
    fiber.begin_loading().unwrap();
    fiber.ready().unwrap();
    fiber.publish().unwrap();
    let services = Services::default().view();
    let registration = services
        .register_method(&fiber.context(), SERVICE, Arc::new(jev))
        .unwrap();
    let handle = services.method(SERVICE).unwrap().unwrap();
    assert!(
        handle
            .call::<_, EvaluationResult>(input(), None, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(t.calls.lock().unwrap().is_empty());
    let parent = scope().await;
    let body = reply(&t, 200, answer());
    let output: EvaluationResult = handle
        .call(input(), Some(parent.clone()), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.usage.input_tokens, 15);
    assert_eq!(body.closes.load(Ordering::SeqCst), 1);
    reply(&t, 200, answer());
    let wire: Value = handle
        .call(
            serde_json::to_value(input()).unwrap(),
            Some(parent.clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(wire["answers"]["done"]["noul"], 0.8);
    {
        let calls = t.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1.url, "https://custom.example/jev");
        assert!(
            calls[0]
                .1
                .headers
                .contains(&("Authorization".into(), "Bearer private-key".into()))
        );
        assert!(
            calls[0]
                .1
                .headers
                .contains(&("X-Tenant".into(), "tenant".into()))
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&calls[0].1.body).unwrap()["model"],
            "custom"
        );
        assert!(calls[0].0.cancellation.is_cancelled());
    }
    assert!(!parent.cancellation.is_cancelled());
    drop(registration);
    assert!(matches!(
        handle
            .call::<_, EvaluationResult>(input(), Some(parent.clone()), CancellationToken::new())
            .await,
        Err(maka_plugins::services::method::Error::Retired)
    ));
    parent.finish().await.unwrap();
}
#[tokio::test]
async fn network_is_bounded_cancelled_and_never_redirected_or_retried() {
    let (jev, t, privacy) = backend();
    let parent = scope().await;
    privacy.0.store(true, Ordering::SeqCst);
    assert!(matches!(
        jev.evaluate(&parent, input()).await,
        Err(Error::Unavailable)
    ));
    assert!(t.calls.lock().unwrap().is_empty());
    privacy.0.store(false, Ordering::SeqCst);
    for (status, bytes) in [(302, vec![]), (429, vec![]), (200, vec![b'a'; 65537])] {
        let body = reply(&t, status, bytes);
        assert!(jev.evaluate(&parent, input()).await.is_err());
        assert_eq!(body.closes.load(Ordering::SeqCst), 1);
    }
    assert_eq!(t.calls.lock().unwrap().len(), 3);
    assert!(matches!(
        jev.evaluate(&parent, input()).await,
        Err(Error::Timeout)
    ));
    assert_eq!(t.calls.lock().unwrap().len(), 4);
    assert!(
        t.calls
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .0
            .cancellation
            .is_cancelled()
    );
    parent.cancellation.cancel();
    assert!(matches!(
        jev.evaluate(&parent, input()).await,
        Err(Error::Cancelled) | Err(Error::Denied)
    ));
    assert_eq!(t.calls.lock().unwrap().len(), 4);
    parent.finish().await.unwrap();
}
#[test]
fn destination_and_header_validation_prevent_credential_confusion() {
    let a = Settings::default();
    let b = Settings {
        url: "https://another.example/jev".into(),
        ..a.clone()
    };
    assert_ne!(a.credential_key(), b.credential_key());
    for url in [
        "file:///tmp/key",
        "https://user:pass@example.com/",
        "https://example.com/#secret",
    ] {
        assert!(
            Settings {
                url: url.into(),
                ..a.clone()
            }
            .validate()
            .is_err()
        );
    }
    for headers in [
        json!({"X-Key":"a","x-key":"b"}),
        json!({"X-Key":"a\r\nInjected: true"}),
        json!({"Host":"other.example"}),
        json!({"Authorization":"custom"}),
    ] {
        let secrets: Secrets =
            serde_json::from_value(json!({"apiKey":"key","headers":headers})).unwrap();
        assert!(secrets.validate().is_err());
    }
    let secrets: Secrets =
        serde_json::from_value(json!({"apiKey":null,"headers":{"Authorization":"Custom key"}}))
            .unwrap();
    assert!(
        secrets
            .request_headers()
            .unwrap()
            .contains(&("Authorization".into(), "Custom key".into()))
    );
}

#[tokio::test]
async fn service_cancellation_and_timeout_preserve_uncertain_post_outcomes() {
    use maka_plugins::services::method::{Context, Error as ServiceError, Method};
    let (jev, transport, _) = backend();
    let parent = scope().await;
    let cancel = CancellationToken::new();
    let request = jev.call(
        input(),
        Context {
            configuration: vec![],
            cancellation: cancel.clone(),
            invocation: Some(parent.clone()),
        },
    );
    let cancellation = async {
        transport.started.notified().await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(request, cancellation);
    assert!(matches!(result, Err(ServiceError::OutcomeUnknown(_))));
    assert!(
        transport.calls.lock().unwrap()[0]
            .0
            .cancellation
            .is_cancelled()
    );
    parent.finish().await.unwrap();
    let parent = scope().await;
    let result = jev
        .call(
            input(),
            Context {
                configuration: vec![],
                cancellation: CancellationToken::new(),
                invocation: Some(parent.clone()),
            },
        )
        .await;
    assert!(matches!(result, Err(ServiceError::OutcomeUnknown(_))));
    parent.finish().await.unwrap();
}
