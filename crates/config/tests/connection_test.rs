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
    connection_test::{ConnectionTestPreparation, PreparedConnectionTest},
    model_fetch::ModelFetchPreparation,
};
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::configuration::*;
use serde_json::{Value, json};
use std::sync::Arc;
#[path = "connection_test/headers.rs"]
mod header_cases;
#[path = "connection_test/network.rs"]
mod network_cases;

fn failed(error_class: ConnectionEffectFailureClass) -> ConnectionTestProjection {
    ConnectionTestProjection::Failed {
        checked_at: "2026-09-12T00:00:00.000Z".into(),
        model_id: Some("private-model-result".into()),
        latency_ms: Some(23),
        status_code: Some(401),
        error_class,
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    namespaces: RootNamespaces,
    store: Arc<ConfigurationStore>,
    id: String,
}

impl Fixture {
    async fn new() -> Self {
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
                "enabledModelIds":["manual"]}
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

    fn input(&self, model: Option<&str>) -> ConnectionTestRunInput {
        ConnectionTestRunInput {
            connection_id: self.id.clone(),
            model_id: model.map(str::to_owned),
        }
    }

    async fn prepare(&self, model: Option<&str>) -> Box<PreparedConnectionTest> {
        match self
            .store
            .prepare_connection_test(self.input(model))
            .await
            .unwrap()
        {
            ConnectionTestPreparation::Ready(prepared) => prepared,
            _ => panic!("expected prepared test"),
        }
    }

    async fn key(&self) {
        assert!(matches!(
            self.store
                .set_credential(
                    SetCredentialInput {
                        locator: CredentialLocator::Connection {
                            connection_id: self.id.clone(),
                            kind: ConnectionCredentialKind::ApiKey,
                        },
                        expected: None,
                        expected_connection: Some(ConnectionCredentialTarget {
                            connection_id: self.id.clone(),
                            revision: 1,
                            slug: "relay".into(),
                            provider_type: "openai-compatible".into(),
                            effective_base_url: "http://127.0.0.1:18080/v1".into(),
                        }),
                        secret: "private-fixture-key".into(),
                    },
                    42
                )
                .await
                .unwrap(),
            CredentialMutationResult::Committed { .. }
        ));
    }

    async fn inventory(&self, model: Value) {
        let ModelFetchPreparation::Ready(prepared) =
            self.store.prepare_model_fetch(&self.id).await.unwrap()
        else {
            panic!("expected prepared discovery")
        };
        assert!(matches!(
            prepared
                .complete(vec![serde_json::from_value(model).unwrap()], 100)
                .await
                .unwrap(),
            ConnectionModelFetchResult::Committed { .. }
        ));
    }

    async fn edit(&self, overlay: Option<Value>) {
        let row = self.store.catalog().await.unwrap().connections.remove(0);
        let mut input = json!({
            "expected":{"connectionId":self.id,"revision":row.revision},
            "changes":{"name":"Renamed during test","baseUrl":row.base_url,
                "enabled":true,"enabledModelIds":row.enabled_model_ids}
        });
        if let Some(overlay) = overlay {
            input["changes"]["requestBodyOverlay"] = overlay;
        }
        assert!(matches!(
            self.store
                .update_connection(serde_json::from_value(input).unwrap())
                .await
                .unwrap(),
            CatalogMutationResult::Committed { .. }
        ));
    }
}

#[tokio::test]
async fn preparation_requires_key_and_admits_manual_or_unselected_discovered_models() {
    let fixture = Fixture::new().await;
    let before = fixture.store.catalog().await.unwrap();
    assert!(matches!(
        fixture
            .store
            .prepare_connection_test(fixture.input(None))
            .await
            .unwrap(),
        ConnectionTestPreparation::Rejected(
            ConnectionEffectRejectionReason::CredentialNotConfigured
        )
    ));
    assert_eq!(fixture.store.catalog().await.unwrap(), before);
    fixture.key().await;
    fixture.inventory(json!({"id":"discovered"})).await;
    let before = fixture.store.catalog().await.unwrap();
    assert!(
        fixture
            .store
            .prepare_connection_test(fixture.input(Some("unknown")))
            .await
            .is_err()
    );
    for model in [None, Some("manual"), Some("discovered")] {
        assert_eq!(fixture.prepare(model).await.model_id(), model);
    }
    assert_eq!(before.connections[0].enabled_model_ids, ["manual"]);
    assert_eq!(fixture.store.catalog().await.unwrap(), before);
}

#[tokio::test]
async fn failed_tests_preserve_rename_and_store_only_safe_summary_across_reopen() {
    let fixture = Fixture::new().await;
    fixture.key().await;
    for (failure, status, stored_class) in [
        (
            ConnectionEffectFailureClass::InvalidResponse,
            ConnectionTestStatus::Error,
            ConnectionTestErrorClass::Unknown,
        ),
        (
            ConnectionEffectFailureClass::Auth,
            ConnectionTestStatus::NeedsReauth,
            ConnectionTestErrorClass::Auth,
        ),
    ] {
        let prepared = fixture.prepare(None).await;
        fixture.edit(None).await;
        let before = fixture.store.catalog().await.unwrap();
        let test = failed(failure);
        assert_eq!(
            prepared.complete(test.clone()).await.unwrap(),
            ConnectionTestRunResult::Committed {
                catalog_revision: before.revision + 1,
                connection: ConnectionVersionBasis {
                    connection_id: fixture.id.clone(),
                    revision: before.connections[0].revision + 1,
                },
                test,
            }
        );
        let mut expected = before;
        expected.revision += 1;
        expected.connections[0].revision += 1;
        expected.connections[0].last_test = Some(ConnectionTestSummary {
            status,
            checked_at: "2026-09-12T00:00:00.000Z".into(),
            error_class: Some(stored_class),
        });
        let saved = fixture.store.catalog().await.unwrap();
        assert_eq!(saved, expected);
        let summary = serde_json::to_value(&saved.connections[0].last_test).unwrap();
        assert_eq!(summary.as_object().unwrap().len(), 3);
        assert!(!serde_json::to_string(&saved).unwrap().contains("private-"));
    }
    let saved = fixture.store.catalog().await.unwrap();
    Arc::try_unwrap(fixture.store)
        .ok()
        .expect("released preparation")
        .close()
        .await
        .unwrap();
    let owner =
        Arc::new(RootOwner::open(&fixture.temp.path().join("root"), &fixture.namespaces).unwrap());
    let reopened = ConfigurationStore::for_root(owner).await.unwrap();
    assert_eq!(reopened.catalog().await.unwrap(), saved);
}

#[tokio::test]
async fn model_metadata_can_change_but_model_wire_source_and_overlay_supersede() {
    let fixture = Fixture::new().await;
    fixture.key().await;
    for model in [
        json!({"id":"manual","apiProtocol":"openai-chat"}),
        json!({"id":"manual","apiProtocol":"openai-responses"}),
    ] {
        let prepared = fixture.prepare(Some("manual")).await;
        fixture.inventory(model).await;
        let before = fixture.store.catalog().await.unwrap();
        assert_eq!(
            prepared
                .complete(failed(ConnectionEffectFailureClass::Auth))
                .await
                .unwrap(),
            ConnectionTestRunResult::Superseded {
                changed: vec![ConnectionEffectChangedDomain::Connection]
            }
        );
        assert_eq!(fixture.store.catalog().await.unwrap(), before);
    }
    let prepared = fixture.prepare(Some("manual")).await;
    fixture
        .inventory(
            json!({"id":"manual","apiProtocol":"openai-responses","displayName":"New title"}),
        )
        .await;
    let before = fixture.store.catalog().await.unwrap();
    let verified = ConnectionTestProjection::Verified {
        checked_at: "2026-09-12T00:00:00.000Z".into(),
        model_id: "manual".into(),
        latency_ms: 8,
    };
    assert!(matches!(
        prepared.complete(verified.clone()).await.unwrap(),
        ConnectionTestRunResult::Committed { .. }
    ));
    let saved = fixture.store.catalog().await.unwrap();
    assert_eq!(saved.connections[0].models, before.connections[0].models);
    assert_eq!(
        saved.connections[0].last_test.as_ref().unwrap().status,
        ConnectionTestStatus::Verified
    );
    for (declaration, superseded) in [
        (
            json!({"displayName":"Configured", "compactionThreshold":8000}),
            false,
        ),
        (json!({"apiProtocol":"openai-chat"}), true),
    ] {
        let prepared = fixture.prepare(Some("manual")).await;
        let row = fixture.store.catalog().await.unwrap().connections.remove(0);
        let input = serde_json::from_value(json!({
            "expected":{"connectionId":row.connection_id,"revision":row.revision},
            "changes":{"name":row.name,"baseUrl":row.base_url,"enabled":row.enabled,
                "enabledModelIds":row.enabled_model_ids,"modelOverrides":{"manual":declaration}}
        }))
        .unwrap();
        fixture.store.update_connection(input).await.unwrap();
        assert_eq!(
            fixture.store.catalog().await.unwrap().connections[0]
                .last_test
                .is_none(),
            superseded,
            "only a changed wire invalidates connection-test evidence"
        );
        let result = prepared.complete(verified.clone()).await.unwrap();
        assert_eq!(
            matches!(result, ConnectionTestRunResult::Superseded { .. }),
            superseded
        );
    }
    let prepared = fixture.prepare(None).await;
    fixture.edit(Some(json!({"temperature":0.5}))).await;
    let before = fixture.store.catalog().await.unwrap();
    assert_eq!(
        prepared.complete(verified).await.unwrap(),
        ConnectionTestRunResult::Superseded {
            changed: vec![ConnectionEffectChangedDomain::Connection]
        }
    );
    assert_eq!(fixture.store.catalog().await.unwrap(), before);
}
