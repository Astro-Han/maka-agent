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

use maka_event_log::{EventLog, StoreError, sessions::ManagedSession};
use maka_plugins::{composition::Scope, storage::Namespace};
use serde_json::{Value, json};

#[tokio::test]
async fn manager_reservations_survive_restart_without_reassigning_or_adopting_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let claim = ManagedSession {
        session_id: "example-inbox".into(),
        manager: Namespace::new("example.workflow", Scope::Profile).unwrap(),
        fingerprint: "owned-creation".into(),
    };
    let log = EventLog::open(&path).await.unwrap();
    log.reserve_managed_session(&claim).await.unwrap();
    log.reserve_managed_session(&claim).await.unwrap();
    assert!(matches!(
        log.create_session(&claim.session_id, "ordinary-create", &json!({}), 1)
            .await,
        Err(StoreError::SessionConflict)
    ));
    let record = log
        .create_session(
            &claim.session_id,
            &claim.fingerprint,
            &json!({"name":"inbox"}),
            2,
        )
        .await
        .unwrap();
    let mut wrong_owner = claim.clone();
    wrong_owner.manager = Namespace::new("other.workflow", Scope::Profile).unwrap();
    assert!(matches!(
        log.reserve_managed_session(&wrong_owner).await,
        Err(StoreError::SessionConflict)
    ));
    let mut wrong_receipt = claim.clone();
    wrong_receipt.fingerprint = "replacement".into();
    assert!(matches!(
        log.reserve_managed_session(&wrong_receipt).await,
        Err(StoreError::SessionConflict)
    ));
    let mut wrong_scope = claim.clone();
    wrong_scope.manager =
        Namespace::new("example.workflow", Scope::Session("other".into())).unwrap();
    assert!(matches!(
        log.reserve_managed_session(&wrong_scope).await,
        Err(StoreError::SessionConflict)
    ));

    // A reservation closes ordinary access even when legacy data is malformed,
    // but does not grant the manager a matching create receipt or rewrite data.
    let legacy = ManagedSession {
        session_id: "legacy".into(),
        ..claim.clone()
    };
    log.create_session(
        "legacy",
        "different-creation",
        &json!({"name":"unrelated"}),
        3,
    )
    .await
    .unwrap();
    log.reserve_managed_session(&legacy).await.unwrap();
    assert!(matches!(
        log.probe_session_create::<Value>("legacy", &legacy.fingerprint)
            .await,
        Err(StoreError::SessionConflict)
    ));
    assert_eq!(
        log.get_session::<Value>("legacy")
            .await
            .unwrap()
            .unwrap()
            .configuration["name"],
        "unrelated"
    );
    log.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.session_manager(&claim.session_id).await.unwrap(),
        Some(claim.manager.clone())
    );
    assert_eq!(
        log.get_session::<Value>(&claim.session_id).await.unwrap(),
        Some(record)
    );
    assert_eq!(log.session_manager("ordinary").await.unwrap(), None);
    log.close().await.unwrap();
}
