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

use maka_config::{ConfigError, ConfigurationStore, oauth::enrollment::*};
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::{
    configuration::*,
    oauth::{LoginStart, Target},
    provider::{AuthenticationInput, Credential, Identity},
    scope::Scope,
};
use serde_json::json;
use sqlx::Connection;
use std::{path::Path, sync::Arc};

fn namespaces(path: &Path) -> RootNamespaces {
    RootNamespaces {
        ownership: path.join("owners"),
        control: path.join("control"),
    }
}
async fn open(path: &Path, create: bool) -> Arc<ConfigurationStore> {
    let root = path.join("root");
    let owner = if create {
        RootOwner::create(&root, &namespaces(path))
    } else {
        RootOwner::open(&root, &namespaces(path))
    }
    .unwrap();
    Arc::new(ConfigurationStore::for_root(Arc::new(owner)).await.unwrap())
}
async fn prepare(store: &Arc<ConfigurationStore>, input: LoginStart) -> PreparedLogin {
    let LoginPreparation::Ready(ticket) = store.prepare_oauth_login(input).await.unwrap() else {
        panic!("expected unpublished login ticket")
    };
    *ticket
}
fn credential(secret: &str) -> Credential {
    Credential {
        secret: secret.into(),
        refresh_at: Some(50_000),
    }
}
fn create(attempt: &str) -> LoginStart {
    LoginStart {
        attempt_id: attempt.into(),
        target: Target::Create {
            provider: Identity {
                package_id: "external.account".into(),
                entry_id: "account-entry".into(),
                scope: Scope::Profile,
                name: "account".into(),
            },
            configuration: json!({"endpoint":"https://account.test/v1"}),
            slug: "chosen-account".into(),
            name: "My account".into(),
        },
        authentication: AuthenticationInput {
            method: "account".into(),
            input: json!({"key":"private-input-a"}),
        },
    }
}
async fn existing(store: &ConfigurationStore, attempt: &str, id: &str) -> LoginStart {
    let row = store
        .catalog()
        .await
        .unwrap()
        .connections
        .into_iter()
        .find(|r| r.connection_id == id)
        .unwrap();
    LoginStart {
        attempt_id: attempt.into(),
        target: Target::Existing {
            expected: ConnectionCredentialTarget {
                connection_id: row.connection_id,
                revision: row.revision,
                slug: row.slug,
                provider: row.provider,
                configuration: row.configuration.clone(),
            },
            configuration: row.configuration,
        },
        authentication: create(attempt).authentication,
    }
}
async fn sql(path: &Path) -> sqlx::SqliteConnection {
    sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path.join("root/configuration-rust.sqlite")),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn authentication_receipts_bind_inputs_and_survive_reopen_without_publishing_drafts() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path(), true).await;
    let input = create("account-login");
    let mut ticket = prepare(&store, input.clone()).await;
    ticket
        .configure_creation(json!({"endpoint":"https://account.test/v1", "oneTimeDefault":true}))
        .unwrap();
    let duplicate = prepare(&store, input.clone()).await;
    let collision = prepare(&store, create("slug-collision")).await;
    assert!(store.catalog().await.unwrap().connections.is_empty());
    assert!(
        store
            .oauth_login_receipt(input.attempt_id.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert!(ticket.claim().await.unwrap());
    assert!(
        !duplicate.claim().await.unwrap(),
        "a claim never permits a second exchange"
    );
    assert!(matches!(
        store.prepare_oauth_login(input.clone()).await.unwrap(),
        LoginPreparation::OutcomeUnknown(_)
    ));
    let LoginCompletion::Committed(receipt) =
        ticket.complete(credential("grant-a"), 1).await.unwrap()
    else {
        panic!("atomic enrollment");
    };
    // A lost commit reply retries persistence, never the grant exchange.
    assert_eq!(
        ticket.complete(credential("grant-a"), 1).await.unwrap(),
        LoginCompletion::Committed(receipt.clone())
    );
    assert_eq!(
        duplicate
            .complete(credential("duplicate"), 1)
            .await
            .unwrap(),
        LoginCompletion::AttemptConflict
    );
    assert_eq!(
        collision
            .complete(credential("collision"), 1)
            .await
            .unwrap(),
        LoginCompletion::SlugTaken
    );
    let snapshot = store.catalog().await.unwrap();
    assert_eq!(snapshot.connections[0], *ticket.connection());
    assert_eq!(
        snapshot.connections[0].provider,
        receipt.connection.provider
    );
    assert_eq!(snapshot.connections[0].slug, "chosen-account");
    assert_eq!(snapshot.connections[0].name, "My account");
    assert_eq!(
        snapshot.connections[0].configuration["oneTimeDefault"],
        true
    );
    drop((ticket, duplicate, collision));
    store.shutdown().await.unwrap();
    drop(store);
    let store = open(temp.path(), false).await;
    assert!(
        matches!(store.prepare_oauth_login(input.clone()).await.unwrap(),
        LoginPreparation::Finished(saved) if saved == receipt)
    );
    let mut different = input.clone();
    different.authentication.input = json!({"key":"private-input-b"});
    assert!(matches!(
        store.prepare_oauth_login(different).await.unwrap(),
        LoginPreparation::Rejected(LoginRejection::AttemptConflict)
    ));
    let mut different = input;
    different.authentication.method = "another-account".into();
    assert!(matches!(
        store.prepare_oauth_login(different).await.unwrap(),
        LoginPreparation::Rejected(LoginRejection::AttemptConflict)
    ));
    assert_eq!(store.catalog().await.unwrap(), snapshot);
    let mut db = sql(temp.path()).await;
    let (target, method, digest): (String, String, String) =
        sqlx::query_as("SELECT target, method, request_fingerprint FROM oauth_login_receipts WHERE attempt_id = 'account-login'")
            .fetch_one(&mut db)
            .await
            .unwrap();
    assert!(!format!("{target}{method}{digest}").contains("private-input"));
    db.close().await.unwrap();
    store.shutdown().await.unwrap();
}

#[tokio::test]
async fn repeated_authentication_retains_only_the_latest_256_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path(), true).await;
    let ticket = prepare(&store, create("initial")).await;
    ticket
        .complete(credential("initial-grant"), 1)
        .await
        .unwrap();
    let id = ticket.identity().connection_id.clone();
    let uncertain = existing(&store, "uncertain-exchange", &id).await;
    let pending = prepare(&store, uncertain.clone()).await;
    assert!(pending.claim().await.unwrap());
    drop(pending);
    let cancelled = existing(&store, "cancelled-login", &id).await;
    let pending = prepare(&store, cancelled.clone()).await;
    assert!(pending.claim().await.unwrap());
    assert_eq!(
        pending
            .finish_failure(maka_runtime::oauth::Phase::Cancelled)
            .await
            .unwrap(),
        maka_runtime::oauth::Phase::Cancelled
    );
    assert!(
        matches!(store.prepare_oauth_login(cancelled).await.unwrap(),
        LoginPreparation::Finished(saved) if saved.phase == maka_runtime::oauth::Phase::Cancelled)
    );
    drop(pending);
    let mut last = None;
    for n in 0..257 {
        let input = existing(&store, &format!("retained-{n}"), &id).await;
        let ticket = prepare(&store, input.clone()).await;
        assert!(matches!(
            ticket
                .complete(credential("replacement-grant"), n)
                .await
                .unwrap(),
            LoginCompletion::Committed(_)
        ));
        last = Some(input);
    }
    assert!(
        store
            .oauth_login_receipt("retained-0".into())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .oauth_login_receipt("retained-1".into())
            .await
            .unwrap()
            .is_some()
    );
    let mut db = sql(temp.path()).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_login_receipts")
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(
        count, 257,
        "bounded completed receipts never evict an unresolved grant"
    );
    db.close().await.unwrap();
    drop(ticket);
    store.shutdown().await.unwrap();
    drop(store);
    let store = open(temp.path(), false).await;
    assert!(
        matches!(
            store.prepare_oauth_login(uncertain).await.unwrap(),
            LoginPreparation::OutcomeUnknown(_)
        ),
        "restart must not retry an uncertain authentication callback"
    );
    assert!(matches!(
        store.prepare_oauth_login(last.unwrap()).await.unwrap(),
        LoginPreparation::Finished(_)
    ));
    assert_eq!(store.catalog().await.unwrap().connections.len(), 1);
    store.shutdown().await.unwrap();
}

#[path = "oauth_login/discovery.rs"]
mod discovery;
#[path = "oauth_login/recovery.rs"]
mod recovery;

#[path = "oauth_login/inventory.rs"]
mod inventory;
