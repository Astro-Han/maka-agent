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

use crate::recap::*;
use futures_util::future::BoxFuture;
use maka_plugins::{
    call,
    execution::{CommandError, Commands},
    llm, preferences,
    session::{catalog, history},
    storage,
};
use maka_runtime::{
    attachment::AttachmentRef,
    model::{ModelFinishReason, ModelGeneration, ModelUsage},
    tools::ToolError,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Default)]
struct Store(Mutex<BTreeMap<String, storage::Record>>);
impl storage::Store for Store {
    fn read(
        &self,
        key: String,
    ) -> BoxFuture<'_, Result<Option<storage::Record>, storage::StoreError>> {
        Box::pin(async move { Ok(self.0.lock().unwrap().get(&key).cloned()) })
    }
    fn scan(&self, _: storage::Scan) -> BoxFuture<'_, Result<storage::Page, storage::StoreError>> {
        Box::pin(async { unreachable!() })
    }
    fn batch(
        &self,
        mutations: Vec<storage::Mutation>,
    ) -> BoxFuture<'_, Result<Vec<storage::Record>, storage::StoreError>> {
        Box::pin(async move {
            let mut records = self.0.lock().unwrap();
            for m in &mutations {
                let actual = records.get(&m.key).map(|r| r.revision);
                if actual != m.expected_revision {
                    return Err(storage::StoreError::Conflict {
                        expected: format!("{:?}", m.expected_revision),
                        actual: format!("{actual:?}"),
                    });
                }
            }
            Ok(mutations
                .into_iter()
                .map(|m| {
                    let r = storage::Record {
                        revision: m.expected_revision.unwrap_or(0) + 1,
                        data: m.data,
                    };
                    records.insert(m.key, r.clone());
                    r
                })
                .collect())
        })
    }
}
#[derive(Default)]
struct Privacy(AtomicBool);
impl preferences::Preferences for Privacy {
    fn read(&self) -> BoxFuture<'_, Result<preferences::Snapshot, maka_plugins::Error>> {
        Box::pin(async {
            Ok(serde_json::from_value(serde_json::json!({"revision":1,"privacy":{"incognitoActive":self.0.load(Ordering::SeqCst)},"personalization":{"displayName":"","assistantTone":""},"workspaceInstructions":true})).unwrap())
        })
    }
}
#[derive(Default)]
struct History {
    endless: AtomicBool,
    denied: AtomicBool,
    reads: AtomicUsize,
}
impl history::History for History {
    fn read(
        &self,
        _: call::Scope,
        input: history::Read,
    ) -> BoxFuture<'_, Result<history::Page, CommandError>> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.denied.load(Ordering::SeqCst) {
                return Err(CommandError::Revoked);
            }
            if input.through == Some(0) {
                return Ok(history::Page::Ready {
                    through: 0,
                    chunks: vec![],
                    next: None,
                });
            }
            assert!(input.through.is_none() || input.through == Some(7));
            let second = input.cursor.is_some();
            let text = if second {
                "Recent result: tests passed; deployment still pending.".into()
            } else {
                "old history 中".repeat(6000)
            };
            Ok(history::Page::Ready {
                through: 7,
                chunks: vec![history::Chunk {
                    message_id: "m".into(),
                    turn_id: "t".into(),
                    timestamp: 1,
                    role: history::Role::Assistant,
                    sequence: if second { 7 } else { 1 },
                    offset: 0,
                    total_bytes: text.len() as u64,
                    text,
                    attachments: vec![],
                }],
                next: (!second || self.endless.load(Ordering::SeqCst)).then_some(history::Cursor {
                    sequence: 2,
                    offset: 0,
                }),
            })
        })
    }
    fn copy_session(
        &self,
        _: call::Scope,
        _: Arc<dyn Commands>,
        _: history::CopySession,
    ) -> BoxFuture<'_, Result<history::CopyResult, CommandError>> {
        Box::pin(async { unreachable!() })
    }
    fn sources(
        &self,
        _: call::Scope,
        _: history::SourcesRead,
    ) -> BoxFuture<'_, Result<Vec<history::EditableMessage>, CommandError>> {
        Box::pin(async { unreachable!() })
    }
    fn list(
        &self,
        _: call::Scope,
        _: catalog::List,
    ) -> BoxFuture<'_, Result<catalog::Page, CommandError>> {
        Box::pin(async { unreachable!() })
    }
    fn copy_material(
        &self,
        _: call::Scope,
        _: Arc<dyn Commands>,
        _: history::CopyMaterial,
    ) -> BoxFuture<'_, Result<AttachmentRef, CommandError>> {
        Box::pin(async { unreachable!() })
    }
}
#[derive(Default)]
struct Models {
    calls: AtomicUsize,
    unknown: AtomicBool,
    wait: AtomicBool,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl llm::Models for Models {
    fn search(&self, _: llm::Search) -> BoxFuture<'_, Result<llm::Choices, maka_plugins::Error>> {
        Box::pin(async { unreachable!() })
    }
    fn resolve(
        &self,
        _: llm::Selection,
    ) -> BoxFuture<'_, Result<Option<llm::Choice>, maka_plugins::Error>> {
        Box::pin(async { unreachable!() })
    }
    fn generate(
        &self,
        scope: call::Scope,
        input: llm::Generate,
    ) -> BoxFuture<'_, Result<ModelGeneration, ToolError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            assert!(input.prompt.len() <= 32 * 1024);
            assert!(input.prompt.contains("deployment still pending"));
            assert_eq!(input.max_output_tokens, Some(1024));
            if self.wait.load(Ordering::SeqCst) {
                tokio::select! {_=self.release.notified()=>{},_=scope.cancellation.cancelled()=>return Err(ToolError::OutcomeUnknown("cancelled".into()))}
            }
            if self.unknown.load(Ordering::SeqCst) {
                return Err(ToolError::OutcomeUnknown("disconnected".into()));
            }
            Ok(ModelGeneration {
                text: "  Tests passed.\nDeployment is next.  ".into(),
                model_id: "model".into(),
                finish_reason: ModelFinishReason::Stop,
                usage: ModelUsage::default(),
            })
        })
    }
}
fn recaps(store: Arc<Store>, history: Arc<History>, models: Arc<Models>) -> Recaps {
    Recaps {
        store,
        history,
        models,
        preferences: Arc::new(Privacy::default()),
    }
}
async fn scope() -> call::Scope {
    call::Issuer::default()
        .admit(
            call::Identity::Remote {
                request_id: Uuid::new_v4(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn durable_retry_and_restart_do_not_regenerate_and_cached_reads_recheck_access() {
    let store = Arc::new(Store::default());
    let history = Arc::new(History::default());
    let models = Arc::new(Models::default());
    let backend = recaps(store.clone(), history.clone(), models.clone());
    let scope = scope().await;
    let id = Uuid::new_v4();
    let result = backend.generate(&scope, "session", id).await.unwrap();
    assert!(
        matches!(result,Receipt::Ready{through:7,ref text,..}if text=="Tests passed. Deployment is next.")
    );
    let restarted = recaps(store, history.clone(), models.clone());
    assert!(matches!(
        restarted.generate(&scope, "session", id).await.unwrap(),
        Receipt::Ready { .. }
    ));
    assert!(matches!(
        restarted.read(&scope, "session").await.unwrap(),
        Some(Receipt::Ready { .. })
    ));
    assert_eq!(models.calls.load(Ordering::SeqCst), 1);
    history.denied.store(true, Ordering::SeqCst);
    assert!(matches!(
        restarted.read(&scope, "session").await,
        Err(Error::History)
    ));
    assert!(matches!(
        restarted.generate(&scope, "session", id).await,
        Err(Error::History)
    ));
    scope.finish().await.unwrap();
}
#[tokio::test]
async fn unknown_operation_is_discoverable_after_restart_and_never_redispatched() {
    let store = Arc::new(Store::default());
    let history = Arc::new(History::default());
    let models = Arc::new(Models::default());
    models.unknown.store(true, Ordering::SeqCst);
    let backend = recaps(store.clone(), history.clone(), models.clone());
    let scope = scope().await;
    let id = Uuid::new_v4();
    assert!(matches!(
        backend.generate(&scope, "session", id).await,
        Err(Error::Unknown)
    ));
    let restarted = recaps(store, history, models.clone());
    assert!(
        matches!(restarted.read(&scope,"session").await.unwrap(),Some(Receipt::Pending{operation_id,..})if operation_id==id)
    );
    assert!(matches!(
        restarted.generate(&scope, "session", id).await.unwrap(),
        Receipt::Pending { .. }
    ));
    assert_eq!(models.calls.load(Ordering::SeqCst), 1);
    scope.finish().await.unwrap();
}
#[tokio::test]
async fn duplicate_pending_operation_and_older_completion_cannot_replace_newer_request() {
    let store = Arc::new(Store::default());
    let history = Arc::new(History::default());
    let models = Arc::new(Models::default());
    models.wait.store(true, Ordering::SeqCst);
    let backend = Arc::new(recaps(store, history, models.clone()));
    let parent = scope().await;
    let first = Uuid::new_v4();
    let worker = {
        let backend = backend.clone();
        let parent = parent.clone();
        tokio::spawn(async move { backend.generate(&parent, "session", first).await })
    };
    models.started.notified().await;
    assert!(matches!(
        backend.generate(&parent, "session", first).await.unwrap(),
        Receipt::Pending { .. }
    ));
    models.wait.store(false, Ordering::SeqCst);
    let second = Uuid::new_v4();
    backend.generate(&parent, "session", second).await.unwrap();
    models.release.notify_one();
    worker.await.unwrap().unwrap();
    assert!(
        matches!(backend.read(&parent,"session").await.unwrap(),Some(Receipt::Ready{operation_id,..})if operation_id==second)
    );
    assert_eq!(models.calls.load(Ordering::SeqCst), 2);
    parent.finish().await.unwrap();
}

#[tokio::test]
async fn oversized_history_and_privacy_refuse_before_model_dispatch() {
    let store = Arc::new(Store::default());
    let history = Arc::new(History::default());
    let models = Arc::new(Models::default());
    history.endless.store(true, Ordering::SeqCst);
    let mut backend = recaps(store.clone(), history.clone(), models.clone());
    let parent = scope().await;
    assert!(matches!(
        backend.generate(&parent, "session", Uuid::new_v4()).await,
        Err(Error::TooLong)
    ));
    assert_eq!(models.calls.load(Ordering::SeqCst), 0);
    assert!(store.0.lock().unwrap().is_empty());
    let private = Arc::new(Privacy::default());
    private.0.store(true, Ordering::SeqCst);
    backend.preferences = private;
    let reads = history.reads.load(Ordering::SeqCst);
    assert!(matches!(
        backend.read(&parent, "session").await,
        Err(Error::Private)
    ));
    assert_eq!(history.reads.load(Ordering::SeqCst), reads);
    parent.finish().await.unwrap();
}

#[tokio::test]
async fn cancellation_settles_model_and_keeps_recoverable_intent() {
    let store = Arc::new(Store::default());
    let history = Arc::new(History::default());
    let models = Arc::new(Models::default());
    models.wait.store(true, Ordering::SeqCst);
    let backend = Arc::new(recaps(store, history, models.clone()));
    let parent = scope().await;
    let id = Uuid::new_v4();
    let worker = {
        let backend = backend.clone();
        let parent = parent.clone();
        tokio::spawn(async move { backend.generate(&parent, "session", id).await })
    };
    models.started.notified().await;
    parent.cancellation.cancel();
    assert!(matches!(worker.await.unwrap(), Err(Error::Unknown)));
    parent.finish().await.unwrap();
    let current = scope().await;
    assert!(
        matches!(backend.read(&current,"session").await.unwrap(),Some(Receipt::Pending{operation_id,..})if operation_id==id)
    );
    assert!(matches!(
        backend.generate(&current, "session", id).await.unwrap(),
        Receipt::Pending { .. }
    ));
    assert_eq!(models.calls.load(Ordering::SeqCst), 1);
    current.finish().await.unwrap();
}
