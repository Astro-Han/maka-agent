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

use crate::health::*;
use futures_util::future::BoxFuture;
use maka_plugins::{call, filesystem, http, preferences};
use maka_runtime::tools::ToolError;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
const REFERENCE: &str = "maka://runtime/background-tasks/task";
struct Files {
    snapshot: Value,
    denied: AtomicBool,
    calls: AtomicUsize,
}
impl filesystem::Files for Files {
    fn invoke(
        &self,
        _: call::Scope,
        operation: filesystem::Operation,
    ) -> BoxFuture<'_, Result<filesystem::Output, ToolError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(matches!(operation,filesystem::Operation::Read(ref r)if r.path==REFERENCE));
            if self.denied.load(Ordering::SeqCst) {
                return Err(ToolError::Failed("Session access denied".into()));
            }
            Ok(filesystem::Output::Value(self.snapshot.clone()))
        })
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
    calls: Mutex<Vec<http::Request>>,
    started: tokio::sync::Notify,
    delay: std::time::Duration,
}
impl http::Client for Transport {
    fn request(
        &self,
        _: call::Scope,
        request: http::Request,
    ) -> BoxFuture<'_, Result<http::Response, http::Error>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(request);
            self.started.notify_one();
            tokio::time::sleep(self.delay).await;
            let result = self.replies.lock().unwrap().pop_front();
            match result {
                Some(r) => Ok(r),
                None => std::future::pending().await,
            }
        })
    }
}
#[derive(Default)]
struct Body {
    cancelled: AtomicBool,
    closed: AtomicUsize,
    uncertain: bool,
}
impl http::Body for Body {
    fn next(&self) -> BoxFuture<'_, Result<Option<Vec<u8>>, http::Error>> {
        Box::pin(async { panic!("Health probes must never read response bodies") })
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst)
    }
    fn close(&self) -> BoxFuture<'_, Result<(), http::Error>> {
        Box::pin(async {
            self.closed.fetch_add(1, Ordering::SeqCst);
            assert!(self.cancelled.load(Ordering::SeqCst));
            if self.uncertain {
                Err(http::Error::CleanupUnconfirmed)
            } else {
                Ok(())
            }
        })
    }
}
fn reply(transport: &Transport, status: u16, uncertain: bool) -> Arc<Body> {
    let body = Arc::new(Body {
        uncertain,
        ..Default::default()
    });
    transport.replies.lock().unwrap().push_back(http::Response {
        head: http::Head {
            status,
            url: "https://example.test/health".into(),
            headers: vec![("Location".into(), b"https://other.test/".to_vec())],
        },
        body: body.clone(),
    });
    body
}
fn backend() -> (Health, Arc<Files>, Arc<Transport>, Arc<Privacy>) {
    let files = Arc::new(Files {
        denied: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        snapshot: json!({"kind":"shell_run","ref":REFERENCE,"mode":"pipes","status":"failed","cwd":"/tmp","cmd":"test","startedAt":1,"updatedAt":2,"completedAt":2,"exitCode":3,"revision":2,"output":{"mode":"pipes","stdout":"private logs","stderr":"failed","stdoutTruncated":false,"stderrTruncated":false,"redacted":false}}),
    });
    let http = Arc::new(Transport::default());
    let privacy = Arc::new(Privacy::default());
    (
        Health {
            files: files.clone(),
            http: http.clone(),
            preferences: privacy.clone(),
        },
        files,
        http,
        privacy,
    )
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
fn input(url: bool, logs: bool) -> Input {
    Input {
        reference: REFERENCE.into(),
        include_logs: logs,
        url: url.then(|| "https://example.test/health".into()),
    }
}
#[tokio::test]
async fn process_and_endpoint_are_independent_logs_are_opt_in_and_bodies_are_discarded() {
    let (health, _, http, _) = backend();
    let scope = scope().await;
    let report =
        serde_json::to_value(health.check(&scope, input(false, false)).await.unwrap()).unwrap();
    assert_eq!(report["process"]["status"], "failed");
    assert_eq!(report["endpoint"]["state"], "not_checked");
    assert!(report["process"].get("logs").is_none());
    assert!(http.calls.lock().unwrap().is_empty());
    let head = reply(&http, 405, false);
    let get = reply(&http, 204, false);
    let report =
        serde_json::to_value(health.check(&scope, input(true, true)).await.unwrap()).unwrap();
    assert_eq!(report["process"]["status"], "failed");
    assert_eq!(report["process"]["exitCode"], 3);
    assert_eq!(report["process"]["logs"]["stdout"], "private logs");
    assert_eq!(report["endpoint"]["health"], "healthy");
    {
        let calls = http.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(matches!(calls[0].method, http::Method::Head));
        assert!(matches!(calls[1].method, http::Method::Get));
    }
    assert_eq!(head.closed.load(Ordering::SeqCst), 1);
    assert_eq!(get.closed.load(Ordering::SeqCst), 1);
    scope.finish().await.unwrap();
}
#[tokio::test]
async fn redirects_are_observations_and_failed_settlement_is_not_a_health_result() {
    let (health, _, http, _) = backend();
    let scope = scope().await;
    for (status, expected) in [(302, "unknown"), (503, "unhealthy")] {
        reply(&http, status, false);
        let report =
            serde_json::to_value(health.check(&scope, input(true, false)).await.unwrap()).unwrap();
        assert_eq!(report["endpoint"]["health"], expected);
    }
    assert_eq!(http.calls.lock().unwrap().len(), 2);
    reply(&http, 200, true);
    assert!(matches!(
        health.check(&scope, input(true, false)).await,
        Err(ToolError::CleanupUnconfirmed(_))
    ));
    scope.finish().await.unwrap();
}
#[tokio::test]
async fn unauthorized_resource_invalid_reference_and_privacy_do_not_probe() {
    let (health, files, http, privacy) = backend();
    let scope = scope().await;
    files.denied.store(true, Ordering::SeqCst);
    assert!(health.check(&scope, input(true, false)).await.is_err());
    assert!(http.calls.lock().unwrap().is_empty());
    files.denied.store(false, Ordering::SeqCst);
    privacy.0.store(true, Ordering::SeqCst);
    let report =
        serde_json::to_value(health.check(&scope, input(true, false)).await.unwrap()).unwrap();
    assert_eq!(report["endpoint"]["state"], "unknown");
    assert!(http.calls.lock().unwrap().is_empty());
    let calls = files.calls.load(Ordering::SeqCst);
    for reference in [
        "/etc/passwd",
        "maka://runtime/background-tasks/%74ask",
        "maka://runtime/background-tasks/task?other=1",
    ] {
        let mut input = input(false, false);
        input.reference = reference.into();
        assert!(health.check(&scope, input).await.is_err());
    }
    assert_eq!(files.calls.load(Ordering::SeqCst), calls);
    scope.finish().await.unwrap();
}
#[tokio::test]
async fn cancellation_drains_probe_and_is_not_reported_as_unhealthy() {
    let (health, _, http, _) = backend();
    let parent = scope().await;
    let check = health.check(&parent, input(true, false));
    let cancel = async {
        http.started.notified().await;
        parent.cancellation.cancel();
    };
    let (result, ()) = tokio::join!(check, cancel);
    assert!(result.is_err());
    parent.finish().await.unwrap();
}

#[tokio::test]
async fn permission_wait_is_not_limited_by_a_plugin_network_deadline() {
    let (mut health, _, _, _) = backend();
    let http = Arc::new(Transport {
        delay: std::time::Duration::from_secs(6),
        ..Default::default()
    });
    reply(&http, 204, false);
    health.http = http;
    let parent = scope().await;
    let report =
        serde_json::to_value(health.check(&parent, input(true, false)).await.unwrap()).unwrap();
    assert_eq!(report["endpoint"]["health"], "healthy");
    parent.finish().await.unwrap();
}
