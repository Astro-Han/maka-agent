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

use super::*;

pub(super) struct Fixture {
    pub(super) temp: tempfile::TempDir,
    pub(super) store: Arc<ConfigurationStore>,
    pub(super) target: ConnectionCredentialTarget,
}
impl Fixture {
    pub(super) async fn new(provider: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let owner = Arc::new(
            RootOwner::create(&temp.path().join("root"), &namespaces(temp.path())).unwrap(),
        );
        let store = Arc::new(ConfigurationStore::for_root(owner).await.unwrap());
        let created = store.create_connection(serde_json::from_value(json!({
            "expectedCatalogRevision":0,
            "connection":{"slug":"subscription","name":"Subscription","providerType":provider,
                "enabled":true,"enabledModelIds":[]}
        })).unwrap()).await.unwrap();
        let CatalogMutationResult::Committed {
            connection: Some(basis),
            ..
        } = created
        else {
            panic!("created connection");
        };
        let target = ConnectionCredentialTarget {
            connection_id: basis.connection_id,
            revision: basis.revision,
            slug: "subscription".into(),
            provider_type: provider.into(),
            effective_base_url: validation::normalize_base_url(
                Some(validation::provider_default_base_url(provider).unwrap()),
                None,
            )
            .unwrap()
            .unwrap(),
        };
        Self {
            temp,
            store,
            target,
        }
    }
    pub(super) async fn login(&self, secret: &str) -> Result<SetCredentialResult, ConfigError> {
        self.store
            .set_credential(
                SetCredentialInput {
                    locator: CredentialLocator::Connection {
                        connection_id: self.target.connection_id.clone(),
                        kind: ConnectionCredentialKind::OauthToken,
                    },
                    expected: None,
                    expected_connection: Some(self.target.clone()),
                    secret: secret.into(),
                },
                10,
            )
            .await
    }
    pub(super) async fn snapshot(&self) -> OAuthCredential {
        self.store
            .oauth_credential(self.target.clone())
            .await
            .unwrap()
            .unwrap()
    }
    pub(super) async fn update(&mut self, enabled: bool) {
        let row = self.store.catalog().await.unwrap().connections.remove(0);
        let result = self
            .store
            .update_connection(UpdateCatalogConnectionInput {
                expected: ConnectionVersionBasis {
                    connection_id: row.connection_id,
                    revision: row.revision,
                },
                changes: ConnectionCatalogEntryUpdate {
                    name: format!("Edited during refresh {}", row.revision),
                    base_url: row.base_url,
                    enabled,
                    enabled_model_ids: row.enabled_model_ids,
                    model_overrides: Patch::Keep,
                    request_body_overlay: Patch::Keep,
                },
            })
            .await
            .unwrap();
        let CatalogMutationResult::Committed {
            connection: Some(basis),
            ..
        } = result
        else {
            panic!("updated connection");
        };
        self.target.revision = basis.revision;
    }
    pub(super) async fn sql(&self) -> sqlx::SqliteConnection {
        sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .filename(self.temp.path().join("root/configuration-rust.sqlite")),
        )
        .await
        .unwrap()
    }
    pub(super) async fn remove(&self) {
        let result = self
            .store
            .remove_connection(RemoveCatalogConnectionInput {
                expected: ConnectionVersionBasis {
                    connection_id: self.target.connection_id.clone(),
                    revision: self.target.revision,
                },
            })
            .await
            .unwrap();
        assert!(matches!(result, CatalogMutationResult::Committed { .. }));
    }
}
pub(super) fn namespaces(path: &std::path::Path) -> RootNamespaces {
    RootNamespaces {
        ownership: path.join("owners"),
        control: path.join("control"),
    }
}
