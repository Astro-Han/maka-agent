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

mod support;

use futures_util::future::BoxFuture;
use maka_event_log::EventLog;
use maka_plugins::storage::{Mutation as StoreMutation, Record, Store, StoreError};
use maka_scheduler::{
    authorization::{Authorization, Origin},
    command::{Mutation, MutationResult},
    controller::Controller,
    delivery::{Clock, Delivery, Dispatcher},
    owner::{self, Handle},
    plan::{Fire, Plan},
    repository::Repository,
    schedule::Schedule,
    task::{Create, Creator, Effect, Notification, Outcome, Status},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::Duration,
};
use support::SqlStore;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

struct ManualClock(AtomicI64);
struct LostSettlement {
    inner: SqlStore,
    armed: Arc<AtomicBool>,
}
impl Store for LostSettlement {
    fn read(&self, key: String) -> BoxFuture<'_, Result<Option<Record>, StoreError>> {
        self.inner.read(key)
    }
    fn batch(
        &self,
        mutations: Vec<StoreMutation>,
    ) -> BoxFuture<'_, Result<Vec<Record>, StoreError>> {
        Box::pin(async move {
            let settlement = mutations
                .iter()
                .filter_map(|mutation| mutation.data.value())
                .any(|value| {
                    value["task"]["fireCount"] == 1
                        && value["task"]["effect"]["kind"] == "session_resume"
                });
            let result = self.inner.batch(mutations).await?;
            if settlement && self.armed.swap(false, Ordering::SeqCst) {
                return Err(StoreError::OutcomeUnknown(
                    "settled; acknowledgement lost".into(),
                ));
            }
            Ok(result)
        })
    }
}
impl Clock for ManualClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct DeliveryHost {
    calls: Mutex<Vec<Fire>>,
    entered: mpsc::UnboundedSender<Fire>,
    release: Notify,
}
impl Dispatcher for DeliveryHost {
    fn authorize(
        &self,
        _: Origin,
        effect: Effect,
    ) -> BoxFuture<'_, Result<Authorization, maka_scheduler::Error>> {
        Box::pin(async move {
            Ok(match effect {
                Effect::Notify(_) => Authorization::Notification { source: None },
                Effect::SessionResume { session_id } => Authorization::Session {
                    boundary: maka_plugins::execution::SessionBoundary {
                        session_id,
                        boundary_revision: 0,
                        permission_mode: maka_runtime::execution::PermissionMode::Ask,
                        cwd: "/fixture".into(),
                    },
                },
                Effect::AgentRun { .. } => panic!("fixture does not provision root Sessions"),
            })
        })
    }
    fn dispatch(&self, fire: Fire, _: CancellationToken) -> BoxFuture<'_, Delivery> {
        Box::pin(async move {
            let previous = {
                let mut calls = self.calls.lock().unwrap();
                let previous = calls.iter().any(|call| call.id == fire.id);
                calls.push(fire.clone());
                previous
            };
            self.entered.send(fire.clone()).unwrap();
            match fire.effect {
                Effect::Notify(_) => Delivery::Notified,
                _ if previous => Delivery::Accepted {
                    session_id: "session".into(),
                    run_id: "run".into(),
                },
                _ => {
                    self.release.notified().await;
                    Delivery::Retry("accepted but reply lost".into())
                }
            }
        })
    }
}
fn input(schedule: Schedule, effect: Effect) -> Create {
    Create {
        title: "Task".into(),
        intent_body: "Work".into(),
        schedule,
        effect,
        max_fires: None,
        expires_at: None,
    }
}
async fn until(handle: &Handle, check: impl Fn(&maka_scheduler::view::View) -> bool) {
    let mut changes = handle.subscribe();
    loop {
        if check(&changes.borrow_and_update()) {
            return;
        }
        changes.changed().await.unwrap();
    }
}
#[tokio::test]
async fn slow_delivery_does_not_block_edits_and_recovery_never_duplicates_unknown_notifications() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("scheduler.sqlite");
        let log = Arc::new(EventLog::open(&path).await.unwrap());
        let fault = Arc::new(AtomicBool::new(true));
        let repository = || {
            Repository::new(
                Arc::new(LostSettlement {
                    inner: SqlStore(log.clone()),
                    armed: fault.clone(),
                }),
                "main",
            )
            .unwrap()
        };
        let controller = Controller::open(repository(), "UTC".into(), 1000)
            .await
            .unwrap();
        let clock = Arc::new(ManualClock(AtomicI64::new(1000)));
        let (entered, mut calls) = mpsc::unbounded_channel();
        let host = Arc::new(DeliveryHost {
            calls: Mutex::default(),
            entered,
            release: Notify::new(),
        });
        let stop = CancellationToken::new();
        let (handle, worker) = owner::start(controller, host.clone(), clock.clone(), stop.clone());
        let worker = tokio::spawn(worker);
        until(&handle, |view| view.ready).await;
        let MutationResult::Task { task } = handle
            .mutate(
                Mutation::Create {
                    input: input(
                        Schedule::Interval {
                            every_seconds: 10,
                            start_at: 1000,
                        },
                        Effect::SessionResume {
                            session_id: "session".into(),
                        },
                    ),
                },
                Origin::User,
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let id = task.id;
        clock.0.store(11_000, Ordering::SeqCst);
        handle.wake();
        let first = calls.recv().await.unwrap();
        let durable = repository().load().await.unwrap();
        assert_eq!(durable.plans[&id].plan.pending.as_ref(), Some(&first));
        handle
            .mutate(
                Mutation::Pause {
                    task_id: id.clone(),
                },
                Origin::User,
            )
            .await
            .unwrap();
        let MutationResult::Task { task: notification } = handle
            .mutate(
                Mutation::Create {
                    input: input(
                        Schedule::Once { run_at: 13_000 },
                        Effect::Notify(Notification::Local),
                    ),
                },
                Origin::User,
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        clock.0.store(13_000, Ordering::SeqCst);
        handle.wake();
        assert!(matches!(
            calls.recv().await.unwrap().effect,
            Effect::Notify(_)
        ));
        until(&handle, |view| view.tasks[&notification.id].fire_count == 1).await;
        assert_eq!(handle.snapshot().tasks[&id].status, Status::Paused);
        host.release.notify_one();
        let retry = calls.recv().await.unwrap();
        assert_eq!(retry, first);
        until(&handle, |view| {
            view.ready && view.tasks[&id].fire_count == 1
        })
        .await;
        assert!(!fault.load(Ordering::SeqCst));
        assert_eq!(handle.snapshot().tasks[&id].status, Status::Paused);
        stop.cancel();
        worker.await.unwrap().unwrap();
        let repository = repository();
        let mut catalog = repository.load().await.unwrap();
        for id in ["unknown", "missed"] {
            let mut plan = Plan::create(
                id.into(),
                input(
                    Schedule::Once { run_at: 14_000 },
                    Effect::Notify(Notification::Local),
                ),
                Creator::User,
                "UTC".into(),
                1000,
            )
            .unwrap();
            if id == "unknown" {
                plan.claim(14_000).unwrap();
                plan.pending.as_mut().unwrap().delivery_started = true;
            }
            repository.save(&mut catalog, plan).await.unwrap();
        }
        drop(repository);
        log.shutdown().await.unwrap();
        drop(log);

        let log = Arc::new(EventLog::open(&path).await.unwrap());
        clock.0.store(15_000, Ordering::SeqCst);
        let controller = Controller::open(
            Repository::new(Arc::new(SqlStore(log.clone())), "main").unwrap(),
            "UTC".into(),
            15_000,
        )
        .await
        .unwrap();
        let stop = CancellationToken::new();
        let (handle, worker) = owner::start(controller, host.clone(), clock, stop.clone());
        let worker = tokio::spawn(worker);
        until(&handle, |view| {
            view.ready && view.tasks["unknown"].fire_count == 1
        })
        .await;
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.tasks["unknown"].runs[0].outcome, Outcome::Blocked);
        assert_eq!(snapshot.tasks["missed"].status, Status::Completed);
        assert_eq!(snapshot.tasks["missed"].fire_count, 0);
        assert_eq!(host.calls.lock().unwrap().len(), 3);
        stop.cancel();
        worker.await.unwrap().unwrap();
        log.shutdown().await.unwrap();
    })
    .await
    .expect("scheduler must make progress while delivery is blocked");
}
