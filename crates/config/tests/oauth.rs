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

use maka_config::{ConfigError, ConfigurationStore, oauth::OAuthCredential};
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::configuration::*;
use serde_json::json;
use sqlx::Connection;
use std::sync::Arc;

#[path = "oauth/fixture.rs"]
mod fixture;
use fixture::{Fixture, namespaces};

#[tokio::test]
async fn refresh_cas_preserves_generation_across_races_logout_aba_and_reopen() {
    let fixture = Fixture::new("github-copilot").await;
    // Use the existing, deliberately allowed Copilot client enrollment path.
    // This is storage acceptance, not evidence of Host OAuth login support.
    fixture.login("old-grant").await.unwrap();
    let original = fixture.snapshot().await;
    let before = fixture.store.catalog().await.unwrap();
    let (first, second) = tokio::join!(
        original.commit_refresh("first-rotation".into(), 11),
        original.commit_refresh("second-rotation".into(), 11),
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_ne!(
        first.is_some(),
        second.is_some(),
        "exactly one writer can rotate this generation"
    );
    let winner = first.or(second).unwrap();
    assert_eq!(winner.credential_id, original.basis().credential_id);
    assert_eq!(winner.revision, original.basis().revision + 1);
    assert_eq!(
        fixture.store.catalog().await.unwrap(),
        before,
        "rotation changes only the vault"
    );
    let current = fixture.snapshot().await;
    assert_eq!(current.basis(), &winner);
    assert!(matches!(
        current.secret(),
        "first-rotation" | "second-rotation"
    ));
    assert_eq!(
        original.secret(),
        "old-grant",
        "admitted snapshots are immutable"
    );
    let public = serde_json::to_string(
        &fixture
            .store
            .credential_status(winner.locator.clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(!public.contains(current.secret()));
    let deleted = fixture
        .store
        .delete_credential(DeleteCredentialInput {
            expected: winner.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(
        deleted,
        CredentialMutationResult::Committed { .. }
    ));
    assert!(
        fixture
            .store
            .oauth_credential(fixture.target.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        current
            .commit_refresh("late-after-logout".into(), 12)
            .await
            .unwrap()
            .is_none()
    );
    // Re-login with the same bytes and revision 1 must not resurrect the old ID.
    fixture.login("old-grant").await.unwrap();
    let relogged = fixture.snapshot().await;
    assert_ne!(
        relogged.basis().credential_id,
        original.basis().credential_id
    );
    assert_eq!(relogged.basis().revision, original.basis().revision);
    assert!(
        original
            .commit_refresh("late-after-relogin".into(), 13)
            .await
            .unwrap()
            .is_none()
    );
    let expected = relogged.basis().clone();
    drop((original, current, relogged));
    let Fixture {
        temp,
        store,
        target,
    } = fixture;
    store.shutdown().await.unwrap();
    drop(store);
    let owner =
        Arc::new(RootOwner::open(&temp.path().join("root"), &namespaces(temp.path())).unwrap());
    let reopened = Arc::new(ConfigurationStore::for_root(owner).await.unwrap());
    let persisted = reopened.oauth_credential(target).await.unwrap().unwrap();
    assert_eq!(persisted.basis(), &expected);
    assert_eq!(persisted.secret(), "old-grant");
    assert_eq!(reopened.catalog().await.unwrap(), before);
    drop(persisted);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn refresh_failure_rolls_back_secret_and_version_and_never_claims_unknown_commit() {
    let fixture = Fixture::new("github-copilot").await;
    fixture.login("valid-grant").await.unwrap();
    let original = fixture.snapshot().await;
    let mut sql = fixture.sql().await;
    sqlx::query("CREATE TRIGGER fail_rotation BEFORE UPDATE ON credential_vault BEGIN SELECT RAISE(ABORT, 'injected vault revision failure'); END")
        .execute(&mut sql).await.unwrap();
    assert!(
        original
            .commit_refresh("rotated-grant".into(), 20)
            .await
            .is_err()
    );
    let current = fixture.snapshot().await;
    assert_eq!(current.basis(), original.basis());
    assert_eq!(current.secret(), original.secret());
    sqlx::query("DROP TRIGGER fail_rotation")
        .execute(&mut sql)
        .await
        .unwrap();
    for invalid in [String::new(), "😀".repeat(32 * 1024) + "x"] {
        assert!(matches!(
            original.commit_refresh(invalid, 20).await,
            Err(ConfigError::Invalid(_))
        ));
    }
    // A deferred constraint fails at COMMIT, not at UPDATE. The caller must
    // receive unknown and reconcile from canonical storage, never use the candidate.
    sqlx::query("CREATE TABLE deferred_failure (parent INTEGER REFERENCES credential_vault(singleton) DEFERRABLE INITIALLY DEFERRED)")
        .execute(&mut sql).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_commit AFTER UPDATE ON credential_vault BEGIN INSERT INTO deferred_failure VALUES(2); END")
        .execute(&mut sql).await.unwrap();
    assert!(matches!(
        original.commit_refresh("not-committed".into(), 21).await,
        Err(ConfigError::CommitUnknown)
    ));
    let reconciled = fixture.snapshot().await;
    assert_eq!(reconciled.basis(), original.basis());
    assert_eq!(reconciled.secret(), original.secret());
    sqlx::query("DROP TRIGGER fail_commit")
        .execute(&mut sql)
        .await
        .unwrap();
    let rotated =
        json!({"access_token":"a".repeat(12 * 1024),"refresh_token":"refresh","expires_at":20000})
            .to_string();
    let basis = original
        .commit_refresh(rotated.clone(), 22)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(basis.revision, original.basis().revision + 1);
    assert_eq!(fixture.snapshot().await.secret(), rotated);
    let vault_revision: i64 = sqlx::query_scalar("SELECT revision FROM credential_vault")
        .fetch_one(&mut sql)
        .await
        .unwrap();
    assert_eq!(vault_revision, 2, "failed writes consume neither version");
    sql.close().await.unwrap();
    fixture.store.shutdown().await.unwrap();
}

#[tokio::test]
async fn refresh_is_bound_to_connection_and_network_snapshot_without_public_codex_import() {
    let mut fixture = Fixture::new("github-copilot").await;
    fixture.login("bound-grant").await.unwrap();
    let original = fixture.snapshot().await;
    let policy = fixture.store.runtime_policy().await.unwrap();
    let mut proxy = policy.policy.network_proxy.clone();
    proxy.enabled = true;
    proxy.host = "127.0.0.1".into();
    proxy.port = 7890;
    fixture
        .store
        .set_network_proxy(policy.revision, proxy.clone())
        .await
        .unwrap();
    assert_eq!(
        original.network_configuration().proxy,
        policy.policy.network_proxy
    );
    assert_eq!(
        fixture.snapshot().await.network_configuration().proxy,
        proxy
    );
    // A completed refresh must retain its rotated grant even when the user
    // changes routing meanwhile. Future requests resolve the new routing snapshot.
    assert!(
        original
            .commit_refresh("routed-grant".into(), 30)
            .await
            .unwrap()
            .is_some()
    );
    let current = fixture.snapshot().await;
    fixture.update(true).await;
    assert!(
        current
            .commit_refresh("after-edit".into(), 31)
            .await
            .unwrap()
            .is_some()
    );
    let current = fixture.snapshot().await;
    assert_eq!(current.secret(), "after-edit");
    fixture.update(false).await;
    let disabled_catalog = fixture.store.catalog().await.unwrap();
    let rotated = current
        .commit_refresh("after-disable".into(), 32)
        .await
        .unwrap()
        .expect("persist a spent refresh grant even when execution is disabled");
    assert_eq!(rotated.revision, current.basis().revision + 1);
    assert_eq!(fixture.store.catalog().await.unwrap(), disabled_catalog);
    assert!(
        fixture
            .store
            .oauth_credential(fixture.target.clone())
            .await
            .unwrap()
            .is_none()
    );
    fixture.update(true).await;
    let current = fixture.snapshot().await;
    assert_eq!(current.secret(), "after-disable");
    assert_eq!(current.basis(), &rotated);
    let mut forged = fixture.target.clone();
    forged.provider_type = "openai-codex".into();
    forged.effective_base_url = validation::provider_default_base_url("openai-codex")
        .unwrap()
        .into();
    assert!(
        fixture
            .store
            .oauth_credential(forged)
            .await
            .unwrap()
            .is_none()
    );
    fixture.remove().await;
    assert!(
        current
            .commit_refresh("after-removal".into(), 33)
            .await
            .unwrap()
            .is_none()
    );
    fixture.store.shutdown().await.unwrap();
    let codex = Fixture::new("openai-codex").await;
    assert!(matches!(
        codex.login("must-not-import").await,
        Err(ConfigError::Invalid(_))
    ));
    assert!(
        codex
            .store
            .oauth_credential(codex.target.clone())
            .await
            .unwrap()
            .is_none()
    );
    codex.store.shutdown().await.unwrap();
}
