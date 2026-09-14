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

use futures_util::{future::join_all, poll};
use maka_runtime::{configuration::*, oauth::Provider};
use maka_runtime_host::oauth::Authority;
use serde_json::Value;
use std::time::Duration;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
#[path = "oauth_refresh/fixture.rs"]
mod fixture;
use fixture::{Fixture, grant};

#[tokio::test]
async fn one_root_shares_spent_grant_survives_abandoned_waiters_and_never_forks_stale_bindings() {
    tokio::time::timeout(Duration::from_secs(15), async {
        for provider in [
            Provider::OpenaiCodex,
            Provider::GithubCopilot,
            Provider::XaiOauth,
        ] {
            let fixture = Fixture::new(provider).await;
            let workers = TaskTracker::new();
            let stop = CancellationToken::new();
            let authority = Authority::new(workers.clone(), stop.clone());
            let binding = authority.bind(fixture.snapshot.clone(), provider).unwrap();
            let first = grant("gho_new", "next", 1).await;
            let mut abandoned = Box::pin(binding.access_token(first.client.clone()));
            assert!(poll!(abandoned.as_mut()).is_pending());
            first.admitted.await.unwrap();
            let mut waiting = (0..16)
                .map(|_| Box::pin(binding.access_token(first.client.clone())))
                .collect::<Vec<_>>();
            for waiter in &mut waiting {
                assert!(poll!(waiter.as_mut()).is_pending());
            }
            drop(abandoned);
            first.release.send(()).unwrap();
            for result in join_all(waiting).await {
                assert_eq!(result.unwrap(), "gho_new");
            }
            assert!(first.server.await.unwrap().contains("refresh_token=old"));
            let current = fixture
                .snapshot
                .current_generation()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                current.basis().revision,
                fixture.snapshot.basis().revision + 1
            );
            assert!(
                authority.bind(fixture.snapshot.clone(), provider).is_err(),
                "stale snapshot must not fork authority"
            );
            let next = authority.bind(current.clone(), provider).unwrap();
            // A subsequent flight uses the NEW invocation's transport. The old
            // proxy has closed; neither an old owner nor old routing can succeed.
            let second = grant("gho_final", "final", 3600).await;
            let mut resolve = Box::pin(next.access_token(second.client.clone()));
            assert!(poll!(resolve.as_mut()).is_pending());
            second.admitted.await.unwrap();
            stop.cancel();
            second.release.send(()).unwrap();
            assert_eq!(resolve.await.unwrap(), "gho_final");
            workers.close();
            workers.wait().await;
            assert!(second.server.await.unwrap().contains("refresh_token=next"));
            let persisted = fixture
                .snapshot
                .current_generation()
                .await
                .unwrap()
                .unwrap();
            let raw: Value = serde_json::from_str(persisted.secret()).unwrap();
            assert_eq!(
                raw["refresh_token"], "final",
                "root drain retains a spent grant"
            );
            assert_eq!(
                persisted.basis().revision,
                fixture.snapshot.basis().revision + 2
            );
            fixture.store.shutdown().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn sql_rollback_and_unknown_commit_retry_only_the_retained_replacement() {
    use sqlx::Connection;
    tokio::time::timeout(Duration::from_secs(15), async {
        for unknown in [false, true] {
            let fixture = Fixture::new(Provider::GithubCopilot).await;
            let mut sql = fixture.sql().await;
            if unknown {
                sqlx::query("CREATE TABLE deferred_failure (parent INTEGER REFERENCES credential_vault(singleton) DEFERRABLE INITIALLY DEFERRED)")
                    .execute(&mut sql).await.unwrap();
                sqlx::query("CREATE TRIGGER fail_rotation AFTER UPDATE ON credential_vault BEGIN INSERT INTO deferred_failure VALUES(2); END")
                    .execute(&mut sql).await.unwrap();
            } else {
                sqlx::query("CREATE TRIGGER fail_rotation BEFORE UPDATE ON credential_vault BEGIN SELECT RAISE(ABORT, 'injected rotation failure'); END")
                    .execute(&mut sql).await.unwrap();
            }
            let workers = TaskTracker::new();
            let authority = Authority::new(workers.clone(), CancellationToken::new());
            let binding = authority.bind(fixture.snapshot.clone(), Provider::GithubCopilot).unwrap();
            let remote = grant("ghu_rotated", "spent-new", 3600).await;
            let mut resolve = Box::pin(binding.access_token(remote.client.clone()));
            assert!(poll!(resolve.as_mut()).is_pending());
            remote.admitted.await.unwrap();
            remote.release.send(()).unwrap();
            assert!(resolve.await.is_err());
            remote.server.await.unwrap();
            assert_eq!(fixture.snapshot.current_generation().await.unwrap().unwrap().basis(), fixture.snapshot.basis());
            sqlx::query("DROP TRIGGER fail_rotation").execute(&mut sql).await.unwrap();
            // The sole provider listener is gone. Success therefore requires
            // retrying SQL with the retained replacement, never the old grant.
            assert_eq!(binding.access_token(remote.client.clone()).await.unwrap(), "ghu_rotated");
            let current = fixture.snapshot.current_generation().await.unwrap().unwrap();
            assert_eq!(serde_json::from_str::<Value>(current.secret()).unwrap()["refresh_token"], "spent-new");
            fixture.store.delete_credential(DeleteCredentialInput { expected: current.basis().clone() }).await.unwrap();
            assert!(binding.access_token(remote.client).await.is_err(), "logout revokes the bound generation");
            assert!(fixture.snapshot.current_generation().await.unwrap().is_none());
            workers.close();
            workers.wait().await;
            sql.close().await.unwrap();
            fixture.store.shutdown().await.unwrap();
        }
    }).await.unwrap();
}
