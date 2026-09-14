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
    access::{AccessCreateMode, AccessCredential, CredentialState},
};
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::access::ManagedPrincipalKind;
use std::sync::Arc;

fn credential(id: &str, digit: &str, pending: bool) -> AccessCredential {
    AccessCredential {
        credential_id: id.into(),
        credential_hash: digit.repeat(64),
        principal_id: "owner".into(),
        principal_kind: ManagedPrincipalKind::RemoteOwner,
        grants: vec![
            "host.status".into(),
            "access.credential.finalize".into(),
            "future.operation".into(),
        ],
        can_publish_client_capabilities: true,
        can_use_host_paths: false,
        created_at: "2026-09-12T00:00:00.000Z".into(),
        capability_owner: None,
        state: if pending {
            CredentialState::Pending {
                expires_at: 100,
                bind_client_instance: true,
            }
        } else {
            CredentialState::Active {
                client_instance_id: None,
            }
        },
    }
}

async fn setup() -> (tempfile::TempDir, RootNamespaces, ConfigurationStore) {
    let temp = tempfile::tempdir().unwrap();
    let namespaces = RootNamespaces {
        ownership: temp.path().join("owners"),
        control: temp.path().join("control"),
    };
    let owner = Arc::new(RootOwner::create(&temp.path().join("root"), &namespaces).unwrap());
    let store = ConfigurationStore::for_root(owner).await.unwrap();
    (temp, namespaces, store)
}

#[tokio::test]
async fn racing_finalizers_bind_once_and_owner_snapshot_survives_revocation_and_reopen() {
    let (temp, namespaces, store) = setup().await;
    let catalog = store.catalog().await.unwrap();
    store
        .create_access_credential(credential("old", "a", false), AccessCreateMode::Issue, None)
        .await
        .unwrap();
    let active = store
        .finalize_access_credential("old".into(), "cannot-bind".into(), None, 0)
        .await
        .unwrap();
    assert!(!active.value.reconnect_required);
    assert!(
        !store
            .has_active_bound_client_identity("owner".into(), "cannot-bind".into())
            .await
            .unwrap()
    );
    let mut provider = credential("provider", "c", false);
    provider.principal_kind = ManagedPrincipalKind::CapabilityProvider;
    provider.principal_id = "provider".into();
    assert!(
        store
            .create_access_credential(
                provider.clone(),
                AccessCreateMode::Issue,
                Some("old".into())
            )
            .await
            .is_err()
    );
    store
        .create_access_credential(
            credential("candidate", "b", true),
            AccessCreateMode::Prepare,
            None,
        )
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        store.finalize_access_credential("candidate".into(), "client-a".into(), None, 99),
        store.finalize_access_credential("candidate".into(), "client-b".into(), None, 99)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let winner = if first.is_ok() {
        "client-a"
    } else {
        "client-b"
    };
    let change = first.or(second).unwrap();
    assert!(change.value.reconnect_required);
    assert_eq!(change.revoked, ["old"]);
    assert!(
        !store
            .finalize_access_credential("candidate".into(), winner.into(), Some(winner.into()), 101)
            .await
            .unwrap()
            .value
            .reconnect_required
    );
    assert!(
        store
            .finalize_access_credential("candidate".into(), winner.into(), None, 101)
            .await
            .unwrap()
            .value
            .reconnect_required
    );
    assert!(
        store
            .has_active_bound_client_identity("owner".into(), winner.into())
            .await
            .unwrap()
    );
    let created = store
        .create_access_credential(provider, AccessCreateMode::Issue, Some("candidate".into()))
        .await
        .unwrap()
        .value;
    assert_eq!(
        created
            .capability_owner
            .as_ref()
            .unwrap()
            .client_instance_id,
        winner
    );
    assert!(
        store
            .revoke_access_credential("candidate".into(), "2026-09-12T01:00:00Z".into())
            .await
            .unwrap()
            .value
    );
    assert!(
        !store
            .has_active_bound_client_identity("owner".into(), winner.into())
            .await
            .unwrap()
    );
    assert_eq!(store.catalog().await.unwrap(), catalog);
    store.close().await.unwrap();
    let reopened = ConfigurationStore::for_root(Arc::new(
        RootOwner::open(&temp.path().join("root"), &namespaces).unwrap(),
    ))
    .await
    .unwrap();
    let authenticated = reopened
        .authenticate_access_credential("c".repeat(64), 500)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(authenticated).unwrap(),
        serde_json::to_value(created).unwrap()
    );
    assert!(
        reopened
            .authenticate_access_credential("b".repeat(64), 500)
            .await
            .unwrap()
            .is_none()
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn expiry_revoke_and_replacement_are_atomic_and_report_removed_credentials() {
    let (_temp, _namespaces, store) = setup().await;
    store
        .create_access_credential(
            credential("active", "a", false),
            AccessCreateMode::Issue,
            None,
        )
        .await
        .unwrap();
    store
        .create_access_credential(credential("p1", "b", true), AccessCreateMode::Prepare, None)
        .await
        .unwrap();
    let replacement = store
        .create_access_credential(credential("p2", "c", true), AccessCreateMode::Prepare, None)
        .await
        .unwrap();
    assert_eq!(replacement.revoked, ["p1"]);
    assert_eq!(
        store.next_access_credential_expiry().await.unwrap(),
        Some(100)
    );
    assert!(
        store
            .authenticate_access_credential("c".repeat(64), 99)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .authenticate_access_credential("c".repeat(64), 100)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .finalize_access_credential("p2".into(), "client".into(), None, 100)
            .await
            .is_err()
    );
    assert_eq!(
        store.active_access_credentials().await.unwrap()[0].credential_id,
        "active"
    );
    assert_eq!(store.expire_access_credentials(100).await.unwrap(), ["p2"]);
    store
        .create_access_credential(credential("p3", "d", true), AccessCreateMode::Prepare, None)
        .await
        .unwrap();
    let revoked = store
        .revoke_access_credential("active".into(), "2026-09-12T01:00:00Z".into())
        .await
        .unwrap();
    assert!(revoked.value);
    assert_eq!(revoked.revoked, ["active", "p3"]);
    assert!(
        !store
            .revoke_access_credential("active".into(), "again".into())
            .await
            .unwrap()
            .value
    );
    // The revoked record remains: neither its identity nor hash can be reissued.
    assert!(
        store
            .create_access_credential(
                credential("active", "a", false),
                AccessCreateMode::Issue,
                None
            )
            .await
            .is_err()
    );
    store
        .create_access_credential(credential("new", "e", false), AccessCreateMode::Issue, None)
        .await
        .unwrap();
    store
        .create_access_credential(credential("p4", "f", true), AccessCreateMode::Prepare, None)
        .await
        .unwrap();
    let replaced = store
        .create_access_credential(
            credential("replacement", "1", false),
            AccessCreateMode::Replace,
            None,
        )
        .await
        .unwrap();
    assert_eq!(replaced.revoked, ["new", "p4"]);
    let mut bad = credential("invalid", "2", true);
    bad.principal_kind = ManagedPrincipalKind::CapabilityProvider;
    assert!(
        store
            .create_access_credential(bad, AccessCreateMode::Prepare, None)
            .await
            .is_err()
    );
    let mut old_shape = serde_json::to_value(credential("legacy", "3", false)).unwrap();
    old_shape.as_object_mut().unwrap().remove("state");
    assert!(serde_json::from_value::<AccessCredential>(old_shape).is_err());
    store.close().await.unwrap();
}
