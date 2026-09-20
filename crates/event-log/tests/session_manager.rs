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
async fn managed_creation_is_atomic_and_cannot_adopt_or_reassign_existing_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let claim = ManagedSession {
        session_id: "example-inbox".into(),
        manager: Namespace::new("example.workflow", Scope::Profile).unwrap(),
        fingerprint: "owned-creation".into(),
    };
    let log = EventLog::open(&path).await.unwrap();
    let configuration = json!({"name":"inbox"});
    let record = log
        .create_managed_session(&claim, &configuration, 1)
        .await
        .unwrap();
    assert_eq!(
        log.create_managed_session(&claim, &configuration, 2)
            .await
            .unwrap(),
        record
    );
    assert!(matches!(
        log.create_session(&claim.session_id, &claim.fingerprint, &configuration, 2)
            .await,
        Err(StoreError::SessionConflict)
    ));
    for manager in [
        Namespace::new("other.workflow", Scope::Profile).unwrap(),
        Namespace::new("example.workflow", Scope::Session("other".into())).unwrap(),
    ] {
        let wrong = ManagedSession {
            manager,
            ..claim.clone()
        };
        assert!(matches!(
            log.create_managed_session(&wrong, &configuration, 2).await,
            Err(StoreError::SessionConflict)
        ));
    }
    let ordinary = ManagedSession {
        session_id: "ordinary".into(),
        ..claim.clone()
    };
    log.create_session("ordinary", &ordinary.fingerprint, &configuration, 1)
        .await
        .unwrap();
    assert!(matches!(
        log.create_managed_session(&ordinary, &configuration, 2)
            .await,
        Err(StoreError::SessionConflict)
    ));
    assert_eq!(log.session_manager("ordinary").await.unwrap(), None);
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.session_manager(&claim.session_id).await.unwrap(),
        Some(claim.manager)
    );
    assert_eq!(
        log.get_session::<Value>(&claim.session_id).await.unwrap(),
        Some(record)
    );
    log.close().await.unwrap();
}
