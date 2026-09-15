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

#[tokio::test]
async fn login_rollback_unknown_commit_and_stale_tickets_never_publish_partial_success() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path(), true).await;
    let input = LoginStart {
        attempt_id: "atomic".into(),
        target: Target::Create {
            provider_type: Provider::OpenaiCodex,
            slug: None,
            name: None,
        },
    };
    let before = store.catalog().await.unwrap();
    let mut sql = sql(temp.path()).await;
    sqlx::query("CREATE TRIGGER fail_receipt BEFORE INSERT ON oauth_login_receipts BEGIN SELECT RAISE(ABORT, 'injected'); END")
        .execute(&mut sql).await.unwrap();
    assert!(
        prepare(&store, input.clone())
            .await
            .complete("grant".into(), 1)
            .await
            .is_err()
    );
    assert_eq!(store.catalog().await.unwrap(), before);
    assert!(
        store
            .oauth_login_receipt("atomic".into())
            .await
            .unwrap()
            .is_none()
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credentials")
        .fetch_one(&mut sql)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "receipt failure rolls back both connection and secret"
    );
    sqlx::query("DROP TRIGGER fail_receipt")
        .execute(&mut sql)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE deferred_failure (parent INTEGER REFERENCES credential_vault(singleton) DEFERRABLE INITIALLY DEFERRED)")
        .execute(&mut sql).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_commit AFTER INSERT ON oauth_login_receipts BEGIN INSERT INTO deferred_failure VALUES(2); END")
        .execute(&mut sql).await.unwrap();
    assert!(matches!(
        prepare(&store, input.clone())
            .await
            .complete("grant".into(), 2)
            .await,
        Err(ConfigError::CommitUnknown)
    ));
    assert!(
        store
            .oauth_login_receipt("atomic".into())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.catalog().await.unwrap(), before);
    sqlx::query("DROP TRIGGER fail_commit")
        .execute(&mut sql)
        .await
        .unwrap();
    let LoginCompletion::Committed(receipt) = prepare(&store, input)
        .await
        .complete("grant".into(), 3)
        .await
        .unwrap()
    else {
        panic!("enrolled")
    };
    let id = &receipt.connection.connection_id;
    let input = existing("credential-race", id);
    let stale = prepare(&store, input.clone()).await;
    let locator = CredentialLocator::Connection {
        connection_id: id.clone(),
        kind: ConnectionCredentialKind::OauthToken,
    };
    let CredentialVaultQueryResult::Status { status } =
        store.credential_status(locator.clone()).await.unwrap()
    else {
        panic!("connection exists")
    };
    let CredentialState::Configured {
        credential_id,
        revision,
        ..
    } = status.state
    else {
        panic!("configured")
    };
    sqlx::query("CREATE TRIGGER fail_relogin BEFORE INSERT ON oauth_login_receipts BEGIN SELECT RAISE(ABORT, 'injected replacement failure'); END")
        .execute(&mut sql).await.unwrap();
    assert!(
        prepare(&store, existing("failed-relogin", id))
            .await
            .complete("different-account".into(), 4)
            .await
            .is_err()
    );
    let restored: (String, i64, String) =
        sqlx::query_as("SELECT credential_id, revision, secret FROM credentials WHERE locator = ?")
            .bind(serde_json::to_string(&locator).unwrap())
            .fetch_one(&mut sql)
            .await
            .unwrap();
    assert_eq!(
        restored,
        (credential_id.clone(), revision as i64, "grant".into()),
        "failed enrollment must restore the old generation and grant"
    );
    assert!(
        store
            .oauth_login_receipt("failed-relogin".into())
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("DROP TRIGGER fail_relogin")
        .execute(&mut sql)
        .await
        .unwrap();
    assert!(matches!(
        store
            .delete_credential(DeleteCredentialInput {
                expected: CredentialVersionBasis {
                    locator: locator.clone(),
                    credential_id,
                    revision,
                }
            })
            .await
            .unwrap(),
        CredentialMutationResult::Committed { .. }
    ));
    assert!(matches!(
        stale.complete("late".into(), 4).await.unwrap(),
        LoginCompletion::Superseded {
            connection: false,
            credential: true
        }
    ));
    assert!(
        store
            .oauth_login_receipt(input.attempt_id)
            .await
            .unwrap()
            .is_none()
    );
    let stale = prepare(&store, existing("connection-race", id)).await;
    let row = store.catalog().await.unwrap().connections.remove(0);
    store
        .update_connection(UpdateCatalogConnectionInput {
            expected: ConnectionVersionBasis {
                connection_id: id.clone(),
                revision: row.revision,
            },
            changes: ConnectionCatalogEntryUpdate {
                name: "User edit".into(),
                enabled: false,
                base_url: row.base_url,
                enabled_model_ids: row.enabled_model_ids,
                model_overrides: Patch::Keep,
                request_body_overlay: Patch::Keep,
            },
        })
        .await
        .unwrap();
    assert!(matches!(
        stale.complete("late".into(), 5).await.unwrap(),
        LoginCompletion::Superseded {
            connection: true,
            credential: false
        }
    ));
    let ticket = prepare(&store, existing("reenable", id)).await;
    assert!(ticket.connection().enabled);
    assert!(matches!(
        ticket.complete("new-grant".into(), 6).await.unwrap(),
        LoginCompletion::Committed(_)
    ));
    assert!(store.catalog().await.unwrap().connections[0].enabled);
    let row = store.catalog().await.unwrap().connections.remove(0);
    store
        .remove_connection(RemoveCatalogConnectionInput {
            expected: ConnectionVersionBasis {
                connection_id: id.clone(),
                revision: row.revision,
            },
        })
        .await
        .unwrap();
    // A receipt is historical completion, not proof that the credential still exists.
    assert_eq!(
        store.oauth_login_receipt("atomic".into()).await.unwrap(),
        Some(receipt)
    );
    assert!(matches!(
        store.credential_status(locator).await.unwrap(),
        CredentialVaultQueryResult::ConnectionNotFound
    ));
    sql.close().await.unwrap();
    store.shutdown().await.unwrap();
}
