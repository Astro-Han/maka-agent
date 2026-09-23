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

use crate::goal::*;
use futures_util::future::BoxFuture;
use maka_plugins::storage;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;
#[derive(Default)]
struct Store(Mutex<BTreeMap<String, storage::Record>>, AtomicBool);
impl storage::Store for Store {
    fn read(
        &self,
        key: String,
    ) -> BoxFuture<'_, Result<Option<storage::Record>, storage::StoreError>> {
        Box::pin(async move { Ok(self.0.lock().unwrap().get(&key).cloned()) })
    }
    fn scan(&self, _: storage::Scan) -> BoxFuture<'_, Result<storage::Page, storage::StoreError>> {
        Box::pin(async {
            Ok(storage::Page {
                entries: self
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(key, _)| key.starts_with("session/"))
                    .map(|(key, record)| storage::Entry {
                        key: key.clone(),
                        record: record.clone(),
                    })
                    .collect(),
                next_after: None,
            })
        })
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
            let committed: Vec<_> = mutations
                .into_iter()
                .map(|m| {
                    let r = storage::Record {
                        revision: m.expected_revision.unwrap_or(0) + 1,
                        data: m.data,
                    };
                    records.insert(m.key, r.clone());
                    r
                })
                .collect();
            if self.1.swap(false, Ordering::SeqCst) {
                return Err(storage::StoreError::OutcomeUnknown(
                    "reply lost after commit".into(),
                ));
            }
            Ok(committed)
        })
    }
}

fn arm() -> Arm {
    Arm {
        operation_id: Uuid::new_v4(),
        objective: "Verify the feature".into(),
        grant: maka_plugins::authorization::Id(Uuid::new_v4()),
        max_iterations: 2,
        token_budget: Some(100),
        start: true,
    }
}
#[tokio::test]
async fn arm_retry_preserves_baseline_and_cannot_replace_or_resurrect_a_goal() {
    let store = Arc::new(Store::default());
    let repo = Repository(store.clone());
    let request = arm();
    let goal = repo
        .arm(
            "session",
            request.clone(),
            Meter {
                known: 10,
                missing: 0,
            },
        )
        .await
        .unwrap();
    let retry = repo
        .arm(
            "session",
            request.clone(),
            Meter {
                known: 99,
                missing: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(retry.baseline.known, 10);
    assert_eq!(retry.id, goal.id);
    let mut different = request.clone();
    different.objective = "different".into();
    assert!(
        repo.arm("session", different, Meter::default())
            .await
            .is_err()
    );
    assert!(repo.arm("session", arm(), Meter::default()).await.is_err());
    let mut saved = repo.read("session").await.unwrap().unwrap();
    saved.goal.status = Status::Achieved;
    repo.save(saved).await.unwrap();
    let new = repo.arm("session", arm(), Meter::default()).await.unwrap();
    assert!(
        repo.arm("session", request, Meter::default())
            .await
            .is_err()
    );
    assert_eq!(repo.read("session").await.unwrap().unwrap().goal.id, new.id);
}
#[tokio::test]
async fn durable_outbox_survives_restart_and_stale_writers_cannot_undo_pause() {
    let store = Arc::new(Store::default());
    let repo = Repository(store.clone());
    repo.arm("session", arm(), Meter::default()).await.unwrap();
    let mut saved = repo.read("session").await.unwrap().unwrap();
    saved.goal.reserve().unwrap();
    let request = saved.goal.pending.as_ref().unwrap().request.clone();
    repo.save(saved).await.unwrap();
    let reopened = Repository(store);
    let mut stale = reopened.read("session").await.unwrap().unwrap();
    let mut control = reopened.read("session").await.unwrap().unwrap();
    control.goal.status = Status::Paused;
    reopened.save(control).await.unwrap();
    stale.goal.pending = None;
    assert!(matches!(
        reopened.save(stale).await,
        Err(Error::Storage(storage::StoreError::Conflict { .. }))
    ));
    let recovered = reopened.read("session").await.unwrap().unwrap();
    assert_eq!(recovered.goal.status, Status::Paused);
    assert_eq!(recovered.goal.pending.unwrap().request, request);
}
#[tokio::test]
async fn iteration_reservations_are_bounded_and_budget_missing_fails_closed() {
    let repo = Repository(Arc::new(Store::default()));
    let mut goal = repo
        .arm(
            "session",
            arm(),
            Meter {
                known: 10,
                missing: 1,
            },
        )
        .await
        .unwrap();
    goal.reserve().unwrap();
    let first = goal.pending.take().unwrap();
    goal.reserve().unwrap();
    let second = goal.pending.take().unwrap();
    assert_ne!(first.request.operation_id, second.request.operation_id);
    goal.reserve().unwrap();
    assert_eq!(goal.status, Status::MaxIterations);
    assert!(goal.pending.is_none());
    assert_eq!(goal.iterations, 2);
    goal.status = Status::Active;
    goal.meter(Meter {
        known: 109,
        missing: 1,
    });
    assert_eq!(goal.status, Status::Active);
    goal.meter(Meter {
        known: 110,
        missing: 1,
    });
    assert_eq!(goal.status, Status::BudgetLimited);
    goal.status = Status::Active;
    goal.meter(Meter {
        known: 11,
        missing: 2,
    });
    assert_eq!(goal.status, Status::BudgetUnknown);
}

#[tokio::test]
async fn lost_storage_reply_recovers_original_arm_and_outbox_without_new_identity() {
    let store = Arc::new(Store::default());
    let repo = Repository(store.clone());
    let request = arm();
    store.1.store(true, Ordering::SeqCst);
    assert!(matches!(
        repo.arm("session", request.clone(), Meter::default()).await,
        Err(Error::Storage(storage::StoreError::OutcomeUnknown(_)))
    ));
    let recovered = repo
        .arm("session", request.clone(), Meter::default())
        .await
        .unwrap();
    assert_eq!(recovered.id, request.operation_id);
    let mut saved = repo.read("session").await.unwrap().unwrap();
    saved.goal.reserve().unwrap();
    let operation = saved
        .goal
        .pending
        .as_ref()
        .unwrap()
        .request
        .operation_id
        .clone();
    store.1.store(true, Ordering::SeqCst);
    assert!(matches!(
        repo.save(saved).await,
        Err(Error::Storage(storage::StoreError::OutcomeUnknown(_)))
    ));
    let mut recovered = repo.read("session").await.unwrap().unwrap().goal;
    assert!(recovered.reserve().is_err());
    assert_eq!(recovered.iterations, 1);
    assert_eq!(recovered.pending.unwrap().request.operation_id, operation);
}

#[tokio::test]
async fn terminal_controls_and_usage_failure_cannot_rewrite_execution_facts() {
    let repo = Repository(Arc::new(Store::default()));
    let mut goal = repo.arm("session", arm(), Meter::default()).await.unwrap();
    goal.reserve().unwrap();
    goal.report = Some(Report {
        status: ReportKind::Achieved,
        note: "Evidence verified".into(),
    });
    goal.settled(
        maka_runtime::event::InvocationOutcome::Completed,
        Err("usage unavailable".into()),
    );
    assert_eq!(goal.status, Status::Achieved);
    assert!(goal.pending.is_none());
    for action in [
        Control::Cancel,
        Control::Complete,
        Control::Pause,
        Control::Resume,
    ] {
        assert!(goal.control(action).is_err());
        assert_eq!(goal.status, Status::Achieved);
    }
    goal.status = Status::Active;
    goal.reserve().unwrap();
    goal.settled(
        maka_runtime::event::InvocationOutcome::Completed,
        Err("usage unavailable".into()),
    );
    assert_eq!(goal.status, Status::BudgetUnknown);
    assert!(goal.pending.is_none());
}
#[tokio::test]
async fn sealed_handoff_and_unknown_cancellation_do_not_pin_background_work() {
    let repo = Repository(Arc::new(Store::default()));
    repo.arm("session", arm(), Meter::default()).await.unwrap();
    let mut saved = repo.read("session").await.unwrap().unwrap();
    saved.goal.reserve().unwrap();
    saved.goal.handoff_paused();
    repo.save(saved).await.unwrap();
    assert!(repo.sessions().await.unwrap().is_empty());
    let mut saved = repo.read("session").await.unwrap().unwrap();
    saved.goal.status = Status::Active;
    saved.goal.reserve().unwrap();
    saved.goal.status = Status::CancellationUnknown;
    saved.goal.pending.as_mut().unwrap().dispatched = true;
    repo.save(saved).await.unwrap();
    assert_eq!(
        repo.sessions().await.unwrap(),
        vec![("session".into(), false)]
    );
}
