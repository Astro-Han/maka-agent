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

struct LostCreation {
    inner: SqlStore,
    armed: AtomicBool,
}
impl Store for LostCreation {
    fn scan(
        &self,
        query: maka_plugins::storage::Scan,
    ) -> BoxFuture<'_, Result<maka_plugins::storage::Page, StoreError>> {
        self.inner.scan(query)
    }
    fn read(&self, key: String) -> BoxFuture<'_, Result<Option<Record>, StoreError>> {
        self.inner.read(key)
    }
    fn batch(
        &self,
        mutations: Vec<StoreMutation>,
    ) -> BoxFuture<'_, Result<Vec<Record>, StoreError>> {
        Box::pin(async move {
            let creation = mutations
                .iter()
                .any(|mutation| mutation.key.contains(":creation:"));
            if creation {
                assert_eq!(
                    mutations.len(),
                    3,
                    "task, index and receipt commit together"
                );
            }
            let receipt = self.inner.batch(mutations).await?;
            if creation && self.armed.swap(false, Ordering::SeqCst) {
                return Err(StoreError::OutcomeUnknown(
                    "creation committed; reply lost".into(),
                ));
            }
            Ok(receipt)
        })
    }
}

#[tokio::test]
async fn creation_receipt_survives_lost_commit_reply_deletion_and_restart_without_reauthorization()
{
    tokio::time::timeout(Duration::from_secs(15), async {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("creation.sqlite");
        let operation_id = uuid::Uuid::new_v4();
        let task_id = format!("task-{operation_id}");
        let create = input(
            Schedule::Once { run_at: 100_000 },
            Effect::Notify(Notification::Local),
        );
        let (entered, _calls) = mpsc::unbounded_channel();
        let dispatcher = Arc::new(DeliveryHost {
            authorizations: AtomicUsize::new(0),
            calls: Mutex::default(),
            entered,
            release: Notify::new(),
        });
        for reopened in [false, true] {
            let log = Arc::new(EventLog::open(&path).await.unwrap());
            let store: Arc<dyn Store> = if reopened {
                Arc::new(SqlStore(log.clone()))
            } else {
                Arc::new(LostCreation {
                    inner: SqlStore(log.clone()),
                    armed: AtomicBool::new(true),
                })
            };
            let now = if reopened { 200_000 } else { 1000 };
            let controller =
                Controller::open(Repository::new(store, "main").unwrap(), "UTC".into(), now)
                    .await
                    .unwrap();
            let stop = CancellationToken::new();
            let (handle, worker) = owner::start(
                controller,
                dispatcher.clone(),
                Arc::new(ManualClock(AtomicI64::new(now))),
                stop.clone(),
            );
            let worker = tokio::spawn(worker);
            until(&handle, |view| view.ready).await;
            if !reopened {
                assert!(handle.creation(operation_id).await.unwrap().is_none());
                let result = handle
                    .mutate(
                        Mutation::CreateOnce {
                            operation_id,
                            input: create.clone(),
                        },
                        Origin::User { grant: None },
                    )
                    .await;
                assert!(matches!(
                    result,
                    Err(maka_scheduler::Error::Storage(StoreError::OutcomeUnknown(
                        _
                    )))
                ));
                until(&handle, |view| view.ready).await;
                assert_eq!(handle.snapshot().tasks.len(), 1);
            } else {
                assert!(handle.snapshot().tasks.is_empty());
            }
            assert_eq!(
                handle.creation(operation_id).await.unwrap(),
                Some(task_id.clone())
            );
            let repeat = || {
                handle.mutate(
                    Mutation::CreateOnce {
                        operation_id,
                        input: create.clone(),
                    },
                    Origin::User { grant: None },
                )
            };
            let (a, b) = tokio::join!(repeat(), repeat());
            for result in [a, b] {
                let MutationResult::Created {
                    operation_id: actual,
                    task_id: actual_task,
                } = result.unwrap()
                else {
                    panic!("receipt")
                };
                assert_eq!(actual, operation_id);
                assert_eq!(actual_task, task_id);
            }
            assert_eq!(
                dispatcher.authorizations.load(Ordering::SeqCst),
                1,
                "committed retries never authorize again"
            );
            let mut changed = create.clone();
            changed.intent_body = "different immutable intent".into();
            assert!(matches!(
                handle
                    .mutate(
                        Mutation::CreateOnce {
                            operation_id,
                            input: changed
                        },
                        Origin::User { grant: None }
                    )
                    .await,
                Err(maka_scheduler::Error::CreationConflict)
            ));
            assert_eq!(dispatcher.authorizations.load(Ordering::SeqCst), 1);
            if !reopened {
                handle
                    .mutate(
                        Mutation::Delete {
                            task_id: task_id.clone(),
                        },
                        Origin::User { grant: None },
                    )
                    .await
                    .unwrap();
            }
            assert!(
                handle.snapshot().tasks.is_empty(),
                "receipt is not permission to resurrect deleted work"
            );
            let other = Controller::open(
                Repository::new(Arc::new(SqlStore(log.clone())), "other").unwrap(),
                "UTC".into(),
                now,
            )
            .await
            .unwrap();
            assert!(other.creation(operation_id).await.unwrap().is_none());
            stop.cancel();
            worker.await.unwrap().unwrap();
            log.shutdown().await.unwrap();
        }
        assert!(dispatcher.calls.lock().unwrap().is_empty());
    })
    .await
    .unwrap();
}
