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

use maka_config::{
    ConfigurationStore,
    onboarding::{OnboardingPreparation, PreparedOnboarding},
};
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::configuration::{onboarding::*, *};
use sqlx::Connection;
use std::sync::Arc;
#[path = "onboarding/defaults.rs"]
mod defaults;

fn input(target: OnboardingTarget) -> OnboardingInput {
    OnboardingInput {
        target,
        api_key: Some("\u{feff}secret\u{feff}".into()),
        base_url: Some("http://127.0.0.1:18080/v1".into()),
    }
}
async fn prepare(
    store: &Arc<ConfigurationStore>,
    input: OnboardingInput,
) -> Box<PreparedOnboarding> {
    match store.prepare_onboarding(input).await.unwrap() {
        OnboardingPreparation::Ready(p) => p,
        _ => panic!("expected prepared onboarding"),
    }
}
#[tokio::test]
async fn onboarding_is_one_atomic_commit_and_stale_discovery_cannot_publish() {
    let temp = tempfile::tempdir().unwrap();
    let namespaces = RootNamespaces {
        ownership: temp.path().join("owners"),
        control: temp.path().join("control"),
    };
    let path = temp.path().join("root");
    let owner = Arc::new(RootOwner::create(&path, &namespaces).unwrap());
    let store = Arc::new(ConfigurationStore::for_root(owner).await.unwrap());
    let target = OnboardingTarget::Create {
        provider_type: "openai-compatible".into(),
        slug: Some("chosen-relay".into()),
        name: Some("My relay".into()),
    };
    let ticket = prepare(&store, input(target.clone())).await;
    assert_eq!(ticket.api_key(), "secret");
    assert!(
        store.catalog().await.unwrap().connections.is_empty(),
        "verification must not publish a draft"
    );
    let mut sql = sqlx::SqliteConnection::connect(&format!(
        "sqlite:{}",
        path.join("configuration-rust.sqlite").display()
    ))
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_onboard BEFORE INSERT ON credentials BEGIN SELECT RAISE(ABORT, 'injected vault failure'); END").execute(&mut sql).await.unwrap();
    assert!(
        ticket
            .complete(vec![ModelInfo::new("one")], vec![], 1)
            .await
            .is_err()
    );
    assert!(
        store.catalog().await.unwrap().connections.is_empty(),
        "credential failure rolls back the connection too"
    );
    let revisions: (i64, i64) =
        sqlx::query_as("SELECT c.revision,v.revision FROM connection_catalog c,credential_vault v")
            .fetch_one(&mut sql)
            .await
            .unwrap();
    assert_eq!(revisions, (0, 0));
    sqlx::query("DROP TRIGGER fail_onboard")
        .execute(&mut sql)
        .await
        .unwrap();
    let first = prepare(&store, input(target.clone())).await;
    let losing = prepare(&store, input(target)).await;
    let OnboardingSaveResult::Saved { connection } = first
        .complete(
            vec![ModelInfo::new("one"), ModelInfo::new("manual")],
            vec![],
            2,
        )
        .await
        .unwrap()
    else {
        panic!("saved");
    };
    assert_eq!(
        losing
            .complete(vec![ModelInfo::new("one")], vec![], 3)
            .await
            .unwrap(),
        OnboardingSaveResult::Rejected {
            reason: OnboardingRejection::SlugTaken
        }
    );
    let catalog = store.catalog().await.unwrap();
    let row = &catalog.connections[0];
    assert_eq!(row.name, "My relay");
    assert_eq!(catalog.default_target.as_ref().unwrap().model_id, "one");
    let locator = CredentialLocator::Connection {
        connection_id: connection.connection_id.clone(),
        kind: ConnectionCredentialKind::ApiKey,
    };
    assert_eq!(
        store
            .credential_secret(&locator, None)
            .await
            .unwrap()
            .as_deref(),
        Some("secret")
    );
    let existing = OnboardingTarget::Existing {
        connection_id: connection.connection_id.clone(),
    };
    let reused = OnboardingInput {
        target: existing.clone(),
        api_key: None,
        base_url: None,
    };
    let ticket = prepare(&store, reused).await;
    assert_eq!(ticket.api_key(), "secret");
    assert!(matches!(
        ticket
            .complete(vec![ModelInfo::new("one")], vec!["one".into()], 4)
            .await
            .unwrap(),
        OnboardingSaveResult::Saved { .. }
    ));
    assert_eq!(
        store.catalog().await.unwrap().connections[0].enabled_model_ids,
        ["one", "manual"]
    );
    let stale = prepare(&store, input(existing.clone())).await;
    let mut changes = store.catalog().await.unwrap().connections.remove(0);
    changes.name = "Concurrent rename".into();
    store
        .update_connection(UpdateCatalogConnectionInput {
            expected: ConnectionVersionBasis {
                connection_id: changes.connection_id.clone(),
                revision: changes.revision,
            },
            changes: ConnectionCatalogEntryUpdate {
                name: changes.name,
                base_url: changes.base_url,
                enabled: true,
                enabled_model_ids: changes.enabled_model_ids,
                model_overrides: Patch::Keep,
                request_body_overlay: Patch::Keep,
            },
        })
        .await
        .unwrap();
    let before = store.catalog().await.unwrap();
    assert_eq!(
        stale
            .complete(vec![ModelInfo::new("new")], vec![], 5)
            .await
            .unwrap(),
        OnboardingSaveResult::Rejected {
            reason: OnboardingRejection::Superseded
        }
    );
    assert_eq!(store.catalog().await.unwrap(), before);
    // Deleting and recreating the same key issues a new credential identity.
    let stale = prepare(&store, input(existing)).await;
    let status = store.credential_status(locator.clone()).await.unwrap();
    let CredentialVaultQueryResult::Status {
        status:
            CredentialStatus {
                state:
                    CredentialState::Configured {
                        credential_id,
                        revision,
                        ..
                    },
                ..
            },
    } = status
    else {
        panic!("credential");
    };
    store
        .delete_credential(DeleteCredentialInput {
            expected: CredentialVersionBasis {
                locator: locator.clone(),
                credential_id,
                revision,
            },
        })
        .await
        .unwrap();
    let row = store.catalog().await.unwrap().connections.remove(0);
    store
        .set_credential(
            SetCredentialInput {
                locator: locator.clone(),
                expected: None,
                expected_connection: Some(ConnectionCredentialTarget {
                    connection_id: row.connection_id.clone(),
                    revision: row.revision,
                    slug: row.slug,
                    provider_type: row.provider_type,
                    effective_base_url: row.base_url.unwrap(),
                }),
                secret: "secret".into(),
            },
            6,
        )
        .await
        .unwrap();
    assert_eq!(
        stale
            .complete(vec![ModelInfo::new("new")], vec![], 7)
            .await
            .unwrap(),
        OnboardingSaveResult::Rejected {
            reason: OnboardingRejection::Superseded
        }
    );
    let before = store.catalog().await.unwrap();
    sql.close().await.unwrap();
    store.shutdown().await.unwrap();
    drop(store);
    let store =
        ConfigurationStore::for_root(Arc::new(RootOwner::open(&path, &namespaces).unwrap()))
            .await
            .unwrap();
    assert_eq!(store.catalog().await.unwrap(), before);
    assert_eq!(
        store
            .credential_secret(&locator, None)
            .await
            .unwrap()
            .as_deref(),
        Some("secret")
    );
    store.close().await.unwrap();
}
