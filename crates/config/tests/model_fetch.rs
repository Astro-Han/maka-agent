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
    model_fetch::{ModelFetchPreparation, PreparedModelFetch},
};
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::configuration::*;
use serde_json::json;
use std::sync::Arc;

struct Fixture {
    temp: tempfile::TempDir,
    namespaces: RootNamespaces,
    store: Arc<ConfigurationStore>,
    id: String,
}

impl Fixture {
    async fn new(selected: &[&str]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let namespaces = RootNamespaces {
            ownership: temp.path().join("owners"),
            control: temp.path().join("control"),
        };
        let owner = Arc::new(RootOwner::create(&temp.path().join("root"), &namespaces).unwrap());
        let store = Arc::new(ConfigurationStore::for_root(owner).await.unwrap());
        let input = serde_json::from_value(json!({
            "expectedCatalogRevision":0,
            "connection":{"slug":"relay","name":"Relay","providerType":"openai-compatible",
                "baseUrl":"http://127.0.0.1:18080/v1","enabled":true,
                "enabledModelIds":selected,"requestBodyOverlay":{"temperature":0.5}}
        }))
        .unwrap();
        let CatalogMutationResult::Committed {
            connection: Some(basis),
            ..
        } = store.create_connection(input).await.unwrap()
        else {
            panic!("expected connection commit")
        };
        Self {
            temp,
            namespaces,
            store,
            id: basis.connection_id,
        }
    }

    async fn row(&self) -> ConnectionCatalogEntry {
        self.store.catalog().await.unwrap().connections.remove(0)
    }

    async fn edit(&self, row: ConnectionCatalogEntry) {
        assert!(matches!(
            self.store
                .update_connection(UpdateCatalogConnectionInput {
                    expected: ConnectionVersionBasis {
                        connection_id: row.connection_id,
                        revision: row.revision
                    },
                    changes: ConnectionCatalogEntryUpdate {
                        name: row.name,
                        base_url: row.base_url,
                        enabled: row.enabled,
                        enabled_model_ids: row.enabled_model_ids,
                        model_overrides: Patch::Keep,
                        request_body_overlay: Patch::Keep,
                    },
                })
                .await
                .unwrap(),
            CatalogMutationResult::Committed { .. }
        ));
    }

    async fn key(&self, secret: &str) {
        let row = self.row().await;
        let locator = CredentialLocator::Connection {
            connection_id: self.id.clone(),
            kind: ConnectionCredentialKind::ApiKey,
        };
        let CredentialVaultQueryResult::Status { status } =
            self.store.credential_status(locator.clone()).await.unwrap()
        else {
            panic!("expected credential status")
        };
        let expected = match status.state {
            CredentialState::Absent => None,
            CredentialState::Configured {
                credential_id,
                revision,
                ..
            } => Some(CredentialIdentityBasis {
                credential_id,
                revision,
            }),
        };
        assert!(matches!(
            self.store
                .set_credential(
                    SetCredentialInput {
                        locator,
                        expected,
                        expected_connection: Some(ConnectionCredentialTarget {
                            connection_id: self.id.clone(),
                            revision: row.revision,
                            slug: row.slug,
                            provider_type: row.provider_type,
                            effective_base_url: row.base_url.unwrap(),
                        }),
                        secret: secret.into(),
                    },
                    42
                )
                .await
                .unwrap(),
            CredentialMutationResult::Committed { .. }
        ));
    }

    async fn prepare(&self) -> Box<PreparedModelFetch> {
        match self.store.prepare_model_fetch(&self.id).await.unwrap() {
            ModelFetchPreparation::Ready(prepared) => prepared,
            _ => panic!("expected ready discovery"),
        }
    }
}

#[tokio::test]
async fn rejected_preparation_and_invalid_completion_leave_catalog_untouched() {
    let fixture = Fixture::new(&[]).await;
    for (id, reason) in [
        (
            "00000000-0000-4000-8000-000000000000",
            ConnectionEffectRejectionReason::ConnectionNotFound,
        ),
        (
            fixture.id.as_str(),
            ConnectionEffectRejectionReason::CredentialNotConfigured,
        ),
    ] {
        assert!(
            matches!(fixture.store.prepare_model_fetch(id).await.unwrap(),
            ModelFetchPreparation::Rejected(actual) if actual == reason)
        );
    }
    let mut row = fixture.row().await;
    row.enabled = false;
    fixture.edit(row).await;
    assert!(matches!(
        fixture
            .store
            .prepare_model_fetch(&fixture.id)
            .await
            .unwrap(),
        ModelFetchPreparation::Rejected(ConnectionEffectRejectionReason::ConnectionDisabled)
    ));
    let mut row = fixture.row().await;
    row.enabled = true;
    fixture.edit(row).await;
    fixture.key("fixture-key").await;
    let before = fixture.store.catalog().await.unwrap();
    for models in [vec![], vec![ModelInfo::new("valid"), ModelInfo::new("")]] {
        assert!(fixture.prepare().await.complete(models, 100).await.is_err());
        assert_eq!(fixture.store.catalog().await.unwrap(), before);
    }
}

#[tokio::test]
async fn completion_preserves_concurrent_metadata_and_choices_across_reopen() {
    let fixture = Fixture::new(&["manual-model"]).await;
    fixture.key("fixture-key").await;
    let target = ConnectionTarget {
        connection_id: fixture.id.clone(),
        model_id: "manual-model".into(),
    };
    assert!(matches!(
        fixture
            .store
            .set_default_target(SetDefaultConnectionTargetInput {
                expected_catalog_revision: 1,
                target: Some(target.clone()),
            })
            .await
            .unwrap(),
        CatalogMutationResult::Committed { .. }
    ));
    let prepared = fixture.prepare().await;
    let mut row = fixture.row().await;
    row.name = "Renamed while fetching".into();
    fixture.edit(row).await;
    let before = fixture.store.catalog().await.unwrap();
    let models = vec![ModelInfo {
        display_name: Some("Discovered".into()),
        ..ModelInfo::new("discovered-model")
    }];
    assert!(matches!(
        prepared.complete(models.clone(), 100).await.unwrap(),
        ConnectionModelFetchResult::Committed {
            model_count: 1,
            fetched_at: 100,
            ..
        }
    ));
    let saved = fixture.store.catalog().await.unwrap();
    let mut expected = before.clone();
    expected.revision += 1;
    expected.connections[0].revision += 1;
    expected.connections[0].models = models;
    expected.connections[0].model_source = Some(ModelDiscoverySource::Fetched);
    expected.connections[0].models_fetched_at = Some(100);
    assert_eq!(saved, expected);
    let store = Arc::try_unwrap(fixture.store)
        .ok()
        .expect("preparation released store");
    store.close().await.unwrap();
    let owner =
        Arc::new(RootOwner::open(&fixture.temp.path().join("root"), &fixture.namespaces).unwrap());
    let reopened = ConfigurationStore::for_root(owner).await.unwrap();
    assert_eq!(reopened.catalog().await.unwrap(), saved);
}

#[tokio::test]
async fn selection_seed_is_one_time_and_changed_fetch_basis_cannot_overwrite_inventory() {
    let fixture = Fixture::new(&[]).await;
    fixture.key("original-key").await;
    assert!(matches!(
        fixture
            .prepare()
            .await
            .complete(vec![ModelInfo::new("first")], 100)
            .await
            .unwrap(),
        ConnectionModelFetchResult::Committed { .. }
    ));
    assert_eq!(fixture.row().await.enabled_model_ids, ["first"]);
    let mut row = fixture.row().await;
    row.enabled_model_ids.clear();
    fixture.edit(row).await;
    assert!(matches!(
        fixture
            .prepare()
            .await
            .complete(vec![ModelInfo::new("second")], 101)
            .await
            .unwrap(),
        ConnectionModelFetchResult::Committed { .. }
    ));
    assert!(fixture.row().await.enabled_model_ids.is_empty());

    let prepared = fixture.prepare().await;
    let mut row = fixture.row().await;
    row.base_url = Some("http://127.0.0.1:18081/v1".into());
    fixture.edit(row).await;
    let before = fixture.store.catalog().await.unwrap();
    assert_eq!(
        prepared
            .complete(vec![ModelInfo::new("stale")], 102)
            .await
            .unwrap(),
        ConnectionModelFetchResult::Superseded {
            changed: vec![ConnectionEffectChangedDomain::Connection]
        }
    );
    assert_eq!(fixture.store.catalog().await.unwrap(), before);

    let prepared = fixture.prepare().await;
    fixture.key("replacement-key").await;
    fixture.key("original-key").await;
    assert_eq!(
        prepared
            .complete(vec![ModelInfo::new("stale")], 103)
            .await
            .unwrap(),
        ConnectionModelFetchResult::Superseded {
            changed: vec![ConnectionEffectChangedDomain::Credential]
        }
    );
    assert_eq!(
        fixture.store.catalog().await.unwrap(),
        before,
        "credential ABA must not revive an old preparation"
    );
}
