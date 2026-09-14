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
    oauth::{LoginStart, Provider, Target},
};
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
fn existing(attempt: &str, connection_id: &str) -> LoginStart {
    LoginStart {
        attempt_id: attempt.into(),
        target: Target::Existing {
            connection_id: connection_id.into(),
        },
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
async fn all_providers_publish_atomically_and_replay_bounded_receipts_after_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path(), true).await;
    let mut last = None;
    for provider in [
        Provider::OpenaiCodex,
        Provider::GithubCopilot,
        Provider::XaiOauth,
    ] {
        let input = LoginStart {
            attempt_id: provider.as_str().into(),
            target: Target::Create {
                provider_type: provider,
                slug: (provider == Provider::OpenaiCodex).then(|| "chosen-codex".into()),
                name: (provider == Provider::OpenaiCodex).then(|| "My Codex".into()),
            },
        };
        let before = store.catalog().await.unwrap();
        let ticket = prepare(&store, input.clone()).await;
        let duplicate = prepare(&store, input.clone()).await;
        let collision = prepare(
            &store,
            LoginStart {
                attempt_id: format!("collision-{}", provider.as_str()),
                target: input.target.clone(),
            },
        )
        .await;
        let identity = ticket.identity().clone();
        let after = ticket.connection().clone();
        if provider == Provider::OpenaiCodex {
            assert_eq!(identity.slug, "chosen-codex");
            assert_eq!(after.name, "My Codex");
        }
        assert!(!after.enabled_model_ids.is_empty());
        assert_eq!(
            store.catalog().await.unwrap(),
            before,
            "no draft publication"
        );
        assert!(
            store
                .oauth_login_receipt(input.attempt_id.clone())
                .await
                .unwrap()
                .is_none()
        );
        let LoginCompletion::Committed(receipt) =
            ticket.complete("synthetic-grant".into(), 1).await.unwrap()
        else {
            panic!("atomic enrollment")
        };
        assert_eq!(receipt.connection, identity);
        assert_eq!(
            duplicate
                .complete("duplicate-grant".into(), 1)
                .await
                .unwrap(),
            LoginCompletion::AttemptConflict
        );
        assert_eq!(
            collision
                .complete("colliding-grant".into(), 1)
                .await
                .unwrap(),
            if provider == Provider::OpenaiCodex {
                LoginCompletion::SlugTaken
            } else {
                LoginCompletion::Superseded {
                    connection: true,
                    credential: false,
                }
            }
        );
        let snapshot = store.catalog().await.unwrap();
        if provider == Provider::OpenaiCodex {
            let taken = LoginStart {
                attempt_id: "already-taken".into(),
                target: input.target.clone(),
            };
            assert!(matches!(
                store.prepare_oauth_login(taken).await.unwrap(),
                LoginPreparation::Rejected(LoginRejection::SlugTaken)
            ));
            assert!(
                store
                    .oauth_login_receipt("collision-openai-codex".into())
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(snapshot.revision, before.revision + 1);
        assert!(snapshot.connections.contains(&after));
        let target = ConnectionCredentialTarget {
            connection_id: identity.connection_id.clone(),
            revision: after.revision,
            slug: identity.slug.clone(),
            provider_type: provider.as_str().into(),
            effective_base_url: validation::normalize_base_url(
                Some(validation::provider_default_base_url(provider.as_str()).unwrap()),
                None,
            )
            .unwrap()
            .unwrap(),
        };
        let credential = store.oauth_credential(target).await.unwrap().unwrap();
        assert_eq!(credential.secret(), "synthetic-grant");
        assert!(
            matches!(store.prepare_oauth_login(input.clone()).await.unwrap(),
            LoginPreparation::Authenticated(saved) if saved == receipt)
        );
        assert_eq!(
            store.catalog().await.unwrap(),
            snapshot,
            "replay writes nothing"
        );
        assert!(matches!(
            store
                .prepare_oauth_login(existing(&input.attempt_id, &identity.connection_id))
                .await
                .unwrap(),
            LoginPreparation::Rejected(LoginRejection::AttemptConflict)
        ));
        last = Some(receipt);
    }
    let receipt = last.unwrap();
    // Repeated re-login is a real supported path; no new connection rows are needed
    // to exercise the bounded receipt log and its eviction order.
    for n in 0..257 {
        let ticket = prepare(
            &store,
            existing(&format!("retained-{n}"), &receipt.connection.connection_id),
        )
        .await;
        assert!(matches!(
            ticket.complete(format!("synthetic-{n}"), n).await.unwrap(),
            LoginCompletion::Committed(_)
        ));
    }
    assert_eq!(store.catalog().await.unwrap().connections.len(), 3);
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
    store.shutdown().await.unwrap();
    drop(store);
    let store = open(temp.path(), false).await;
    let saved = store
        .oauth_login_receipt("retained-256".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.connection, receipt.connection);
    assert!(matches!(
        store
            .prepare_oauth_login(existing("retained-256", &receipt.connection.connection_id))
            .await
            .unwrap(),
        LoginPreparation::Authenticated(_)
    ));
    let mut sql = sql(temp.path()).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_login_receipts")
        .fetch_one(&mut sql)
        .await
        .unwrap();
    assert_eq!(count, 256);
    sql.close().await.unwrap();
    store.shutdown().await.unwrap();
}

#[path = "oauth_login/recovery.rs"]
mod recovery;

#[path = "oauth_login/discovery.rs"]
mod discovery;
