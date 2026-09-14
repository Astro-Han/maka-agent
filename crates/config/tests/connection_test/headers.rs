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
use maka_runtime::configuration::headers::*;
use sqlx::Connection;

#[tokio::test]
async fn header_replacement_rolls_back_secret_test_and_revisions_and_preserves_aba() {
    let fixture = Fixture::new().await;
    fixture.key().await;
    let input = |headers| {
        serde_json::from_value(json!({"connectionId":fixture.id,"headers":headers})).unwrap()
    };
    let replace = |headers| fixture.store.replace_request_headers(input(headers), 123);
    let locator = CredentialLocator::Connection {
        connection_id: fixture.id.clone(),
        kind: ConnectionCredentialKind::RequestHeaders,
    };
    assert!(matches!(
        replace(json!([{"name":"X-Keep","value":"old"},{"name":"X-Drop","value":"drop"}]))
            .await
            .unwrap(),
        RequestHeadersReplaceResult::Committed { .. }
    ));
    let before_status = fixture
        .store
        .credential_status(locator.clone())
        .await
        .unwrap();
    let secret = fixture
        .store
        .credential_secret(&locator, None)
        .await
        .unwrap();
    fixture
        .prepare(None)
        .await
        .complete(failed(ConnectionEffectFailureClass::Auth))
        .await
        .unwrap();
    let before = fixture.store.catalog().await.unwrap();
    let prepared = fixture.prepare(None).await;
    let mut sql = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.temp.path().join("root/configuration-rust.sqlite")),
    )
    .await
    .unwrap();
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM credential_vault")
        .fetch_one(&mut sql)
        .await
        .unwrap();
    // Cuts both after secret write/delete and after lastTest invalidation.
    for statement in [
        "CREATE TRIGGER header_cut BEFORE UPDATE ON connections BEGIN SELECT RAISE(ABORT, 'injected'); END",
        "CREATE TRIGGER header_cut BEFORE UPDATE ON credential_vault BEGIN SELECT RAISE(ABORT, 'injected'); END",
    ] {
        sqlx::query(statement).execute(&mut sql).await.unwrap();
        for headers in [json!([{"name":"X-Keep","value":"new"}]), json!([])] {
            assert!(replace(headers).await.is_err());
            assert_eq!(fixture.store.catalog().await.unwrap(), before);
            assert_eq!(
                fixture
                    .store
                    .credential_status(locator.clone())
                    .await
                    .unwrap(),
                before_status
            );
            assert_eq!(
                fixture
                    .store
                    .credential_secret(&locator, None)
                    .await
                    .unwrap(),
                secret
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT revision FROM credential_vault")
                    .fetch_one(&mut sql)
                    .await
                    .unwrap(),
                revision
            );
        }
        sqlx::query("DROP TRIGGER header_cut")
            .execute(&mut sql)
            .await
            .unwrap();
    }
    assert!(matches!(
        replace(json!([{"name":"X-Keep"},{"name":"X-Drop"}]))
            .await
            .unwrap(),
        RequestHeadersReplaceResult::Unchanged { .. }
    ));
    assert_eq!(fixture.store.catalog().await.unwrap(), before);
    assert!(replace(json!([{"name":"X-New"}])).await.is_err());
    assert_eq!(
        fixture
            .store
            .credential_status(locator.clone())
            .await
            .unwrap(),
        before_status
    );
    assert!(matches!(
        replace(json!([{"name":"x-keep"}])).await.unwrap(),
        RequestHeadersReplaceResult::Committed { .. }
    ));
    assert_eq!(
        fixture
            .store
            .credential_secret(&locator, None)
            .await
            .unwrap()
            .unwrap(),
        r#"{"x-keep":"old"}"#
    );
    assert!(
        fixture.store.catalog().await.unwrap().connections[0]
            .last_test
            .is_none()
    );
    assert_eq!(
        prepared
            .complete(failed(ConnectionEffectFailureClass::Auth))
            .await
            .unwrap(),
        ConnectionTestRunResult::Superseded {
            changed: vec![ConnectionEffectChangedDomain::Credential]
        }
    );
    let pending = fixture.prepare(None).await;
    assert!(matches!(
        replace(json!([])).await.unwrap(),
        RequestHeadersReplaceResult::Committed { .. }
    ));
    assert!(matches!(
        replace(json!([])).await.unwrap(),
        RequestHeadersReplaceResult::Unchanged { .. }
    ));
    assert!(matches!(
        replace(json!([{"name":"X-Keep","value":"old"},{"name":"X-Drop","value":"drop"}]))
            .await
            .unwrap(),
        RequestHeadersReplaceResult::Committed { .. }
    ));
    assert_ne!(
        fixture
            .store
            .credential_status(locator.clone())
            .await
            .unwrap(),
        before_status
    );
    assert_eq!(
        pending
            .complete(failed(ConnectionEffectFailureClass::Auth))
            .await
            .unwrap(),
        ConnectionTestRunResult::Superseded {
            changed: vec![ConnectionEffectChangedDomain::Credential]
        }
    );
    // Retention is validated against the resulting aggregate, not just the supplied values.
    let huge: Vec<_> = (0..5)
        .map(|i| json!({"name":format!("X-{i}"),"value":"ÿ".repeat(8192)}))
        .collect();
    assert!(replace(json!(huge[..3])).await.is_ok());
    let before = fixture
        .store
        .credential_status(locator.clone())
        .await
        .unwrap();
    let mut retained: Vec<_> = (0..3).map(|i| json!({"name":format!("X-{i}")})).collect();
    retained.extend_from_slice(&huge[3..]);
    assert!(replace(json!(retained)).await.is_err());
    assert_eq!(
        fixture
            .store
            .credential_status(locator.clone())
            .await
            .unwrap(),
        before
    );
    let saved = fixture
        .store
        .credential_secret(&locator, None)
        .await
        .unwrap()
        .unwrap();
    let locator_json = serde_json::to_string(&locator).unwrap();
    // Corrupt saved data cannot become an empty map or be overwritten by replacement.
    for invalid in [
        r#"[]"#,
        r#"{"X-Count":1}"#,
        r#"{"X-Keep":"old","x-keep":"ambiguous"}"#,
        r#"{"Host":"forbidden"}"#,
    ] {
        sqlx::query("UPDATE credentials SET secret = ? WHERE locator = ?")
            .bind(invalid)
            .bind(&locator_json)
            .execute(&mut sql)
            .await
            .unwrap();
        assert!(
            fixture
                .store
                .request_headers(fixture.id.clone())
                .await
                .is_err()
        );
        assert!(replace(json!([])).await.is_err());
        assert_eq!(
            fixture
                .store
                .credential_status(locator.clone())
                .await
                .unwrap(),
            before
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT secret FROM credentials WHERE locator = ?")
                .bind(&locator_json)
                .fetch_one(&mut sql)
                .await
                .unwrap(),
            invalid
        );
    }
    sqlx::query("UPDATE credentials SET secret = ? WHERE locator = ?")
        .bind(saved)
        .bind(locator_json)
        .execute(&mut sql)
        .await
        .unwrap();
    sql.close().await.unwrap();
}
