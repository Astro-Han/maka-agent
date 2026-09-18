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

use maka_config::ConfigurationStore;
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::configuration::*;
use rusqlite::{Connection, params};
use serde_json::json;
use std::{path::Path, sync::Arc};

fn private_database(root: &Path) -> Connection {
    let path = root.join("configuration-rust.sqlite");
    #[cfg(windows)]
    maka_event_log::root::windows::create_private_file(&path).unwrap();
    #[cfg(not(windows))]
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path).unwrap();
    }
    Connection::open(path).unwrap()
}

fn root(temp: &Path) -> Arc<RootOwner> {
    Arc::new(
        RootOwner::create(
            &temp.join("root"),
            &RootNamespaces {
                ownership: temp.join("owners"),
                control: temp.join("control"),
            },
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn legacy_catalog_and_vault_migrate_without_losing_revisions_or_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let owner = root(temp.path());
    let database = private_database(owner.canonical_path());
    // This is the previous Rust schema, deliberately independent of the new migration.
    database
        .execute_batch(
            "
        PRAGMA application_id = 1296124739;
        PRAGMA user_version = 1;
        CREATE TABLE connection_catalog (
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
            revision INTEGER NOT NULL CHECK(revision >= 0), default_target TEXT);
        CREATE TABLE connections (
            connection_id TEXT PRIMARY KEY, slug TEXT NOT NULL UNIQUE,
            revision INTEGER NOT NULL CHECK(revision > 0), document TEXT NOT NULL);
        CREATE TABLE credential_vault (
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
            revision INTEGER NOT NULL CHECK(revision >= 0));
        CREATE TABLE credentials (
            locator TEXT PRIMARY KEY, credential_id TEXT NOT NULL UNIQUE,
            revision INTEGER NOT NULL CHECK(revision > 0),
            secret TEXT NOT NULL, updated_at INTEGER NOT NULL);
        INSERT INTO credential_vault VALUES(1, 9);
    ",
        )
        .unwrap();
    let connection_id = "11111111-1111-4111-8111-111111111111".to_owned();
    let target = ConnectionTarget {
        connection_id: connection_id.clone(),
        model_id: "fixture".into(),
    };
    let mut entry: ConnectionCatalogEntry = serde_json::from_value(json!({
        "connectionId":connection_id, "revision":3, "slug":"legacy", "name":"Legacy",
        "providerType":"openai-compatible", "baseUrl":"http://127.0.0.1:18080/v1",
        "enabled":true, "enabledModelIds":["fixture"], "models":[]
    }))
    .unwrap();
    let mut legacy_document = serde_json::to_value(&entry).unwrap();
    legacy_document["relayModelProfiles"] = json!({
        "fixture":{"contextWindow":8192,"vision":true}
    });
    legacy_document["lastTest"] = json!({"status":"verified","checkedAt":"legacy"});
    legacy_document["lastTestModelFactsFingerprint"] = json!("sha256:old-file-basis");
    let external = owner.canonical_path().join("model-facts.json");
    std::fs::write(&external, b"not a Rust configuration source").unwrap();
    entry.model_overrides = Some(
        serde_json::from_value(json!({
            "fixture":{"contextWindow":8192,"compactionThreshold":8192,"vision":true}
        }))
        .unwrap(),
    );
    database
        .execute(
            "INSERT INTO connection_catalog VALUES(1, 7, ?)",
            [serde_json::to_string(&target).unwrap()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO connections VALUES(?, 'legacy', 3, ?)",
            params![
                connection_id,
                serde_json::to_string(&legacy_document).unwrap()
            ],
        )
        .unwrap();
    let key = CredentialLocator::Connection {
        connection_id: connection_id.clone(),
        kind: ConnectionCredentialKind::ApiKey,
    };
    let unrelated = CredentialLocator::NetworkProxy {
        kind: PasswordKind::Password,
    };
    for (locator, id, secret) in [
        (
            &key,
            "22222222-2222-4222-8222-222222222222",
            "legacy-private-key",
        ),
        (
            &unrelated,
            "33333333-3333-4333-8333-333333333333",
            "legacy-proxy-password",
        ),
    ] {
        database
            .execute(
                "INSERT INTO credentials VALUES(?, ?, 4, ?, 42)",
                params![serde_json::to_string(locator).unwrap(), id, secret],
            )
            .unwrap();
    }
    drop(database);

    let store = ConfigurationStore::for_root(owner.clone()).await.unwrap();
    let snapshot = store.catalog().await.unwrap();
    assert_eq!(
        std::fs::read(&external).unwrap(),
        b"not a Rust configuration source"
    );
    assert_eq!(snapshot.revision, 7);
    assert_eq!(snapshot.connections, vec![entry]);
    assert_eq!(snapshot.default_target, Some(target));
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("legacy-private-key")
    );
    assert_eq!(
        store
            .credential_secret(&key, None)
            .await
            .unwrap()
            .as_deref(),
        Some("legacy-private-key")
    );
    let status = store.credential_status(key.clone()).await.unwrap();
    assert!(matches!(
        &status,
        CredentialVaultQueryResult::Status {
            status: CredentialStatus {
                state: CredentialState::Configured {
                    revision: 4,
                    updated_at: 42,
                    ..
                },
                ..
            }
        }
    ));
    let basis = ConnectionVersionBasis {
        connection_id,
        revision: 3,
    };
    let mut stale = basis.clone();
    stale.revision = 2;
    assert_eq!(
        store
            .remove_connection(RemoveCatalogConnectionInput {
                expected: stale.clone()
            })
            .await
            .unwrap(),
        CatalogMutationResult::ConnectionStale {
            expected: stale,
            actual: Some(basis.clone())
        }
    );
    assert!(matches!(
        store
            .delete_credential(DeleteCredentialInput {
                expected: CredentialVersionBasis {
                    locator: key.clone(),
                    credential_id: "22222222-2222-4222-8222-222222222222".into(),
                    revision: 3
                }
            })
            .await
            .unwrap(),
        CredentialMutationResult::CredentialStale { .. }
    ));
    assert_eq!(store.catalog().await.unwrap(), snapshot);
    assert_eq!(store.credential_status(key.clone()).await.unwrap(), status);
    store.close().await.unwrap();

    let store = ConfigurationStore::for_root(owner.clone()).await.unwrap();
    assert_eq!(store.catalog().await.unwrap(), snapshot);
    assert_eq!(store.credential_status(key.clone()).await.unwrap(), status);
    assert_eq!(
        store
            .remove_connection(RemoveCatalogConnectionInput { expected: basis })
            .await
            .unwrap(),
        CatalogMutationResult::Committed {
            catalog_revision: 8,
            connection: None
        }
    );
    assert_eq!(store.catalog().await.unwrap().default_target, None);
    assert_eq!(
        store
            .credential_secret(&unrelated, None)
            .await
            .unwrap()
            .as_deref(),
        Some("legacy-proxy-password")
    );
    store.close().await.unwrap();
    let database =
        Connection::open(owner.canonical_path().join("configuration-rust.sqlite")).unwrap();
    assert_eq!(
        database
            .query_row(
                "SELECT revision, (SELECT COUNT(*) FROM credentials) FROM credential_vault",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            )
            .unwrap(),
        (10, 1)
    );
    assert_eq!(
        database
            .query_row(
                "SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn invalid_databases_are_rejected_without_mutation_and_interrupted_initialization_resumes() {
    for corruption in [
        "UPDATE _sqlx_migrations SET checksum = X'00'",
        "UPDATE _sqlx_migrations SET version = version + 999",
        "PRAGMA application_id = 123",
        "PRAGMA user_version = 999",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let owner = root(temp.path());
        let store = ConfigurationStore::for_root(owner.clone()).await.unwrap();
        store.close().await.unwrap();
        let path = owner.canonical_path().join("configuration-rust.sqlite");
        let database = Connection::open(&path).unwrap();
        database.execute_batch(corruption).unwrap();
        drop(database);
        let before = std::fs::read(&path).unwrap();
        assert!(
            ConfigurationStore::for_root(owner.clone()).await.is_err(),
            "{corruption}"
        );
        assert!(std::fs::read(&path).unwrap() == before, "{corruption}");
    }
    let temp = tempfile::tempdir().unwrap();
    let owner = root(temp.path());
    let database = private_database(owner.canonical_path());
    database
        .execute_batch("PRAGMA application_id = 1296124739;")
        .unwrap();
    drop(database);
    let store = ConfigurationStore::for_root(owner).await.unwrap();
    assert_eq!(store.catalog().await.unwrap().revision, 0);
    assert!(store.catalog().await.unwrap().connections.is_empty());
    store.close().await.unwrap();
}
