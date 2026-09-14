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

use super::support::client_probe::ClientFixture;
use maka_runtime::event::{Fact, InvocationOutcome};
use sqlx::Connection;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_interrupt_retracts_once_waits_for_cleanup_and_replays_exactly() {
    let fixture = ClientFixture::new("maka-message-interrupt-");
    fixture
        .run(
            "--message-interrupt-workspace",
            false,
            "message-interrupt-passed",
        )
        .await;
    let log = fixture.log().await;
    let prefix = log.prefix(1000, 8 * 1024 * 1024).await.unwrap();
    assert_eq!(
        prefix
            .events
            .iter()
            .filter(|event| matches!(event.event.fact, Fact::InvocationOpened { .. }))
            .count(),
        3
    );
    assert_eq!(
        prefix
            .events
            .iter()
            .filter(|event| matches!(
                event.event.fact,
                Fact::InvocationEnded {
                    outcome: InvocationOutcome::Cancelled { .. }
                }
            ))
            .count(),
        1
    );
    assert!(
        log.pending_messages("message-interrupt")
            .await
            .unwrap()
            .is_empty()
    );
    for id in ["steer", "next"] {
        assert!(
            log.message_cancelled("message-interrupt", id)
                .await
                .unwrap()
        );
        assert!(
            log.root_message("message-interrupt", id)
                .await
                .unwrap()
                .is_none()
        );
    }
    let bytes = serde_json::to_vec(&prefix).unwrap();
    log.close().await.unwrap();
    fixture
        .run(
            "--message-interrupt-workspace",
            true,
            "message-interrupt-reopened",
        )
        .await;
    let log = fixture.log().await;
    assert_eq!(
        serde_json::to_vec(&log.prefix(1000, 8 * 1024 * 1024).await.unwrap()).unwrap(),
        bytes
    );
    log.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_interrupt_reports_unknown_when_terminal_commit_fails() {
    let fixture = ClientFixture::new("maka-interrupt-fault-");
    fixture.log().await.close().await.unwrap();
    let owner = fixture.owner();
    let path = owner
        .canonical_path()
        .join(maka_event_log::root::ROOT_DATABASE);
    drop(owner);
    let mut db = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(path),
    )
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_cancel_terminal BEFORE INSERT ON event_log
         WHEN NEW.kind = 'invocation_ended' AND json_extract(NEW.event_json, '$.fact.outcome.kind') = 'cancelled'
         BEGIN SELECT RAISE(ABORT, 'terminal fault'); END;"
    ).execute(&mut db).await.unwrap();
    db.close().await.unwrap();
    fixture
        .run(
            "--message-interrupt-failure-workspace",
            false,
            "message-interrupt-failure-passed",
        )
        .await;
    let log = fixture.log().await;
    let prefix = log.prefix(1000, 8 * 1024 * 1024).await.unwrap();
    assert_eq!(
        prefix
            .events
            .iter()
            .filter(|event| matches!(event.event.fact, Fact::InvocationOpened { .. }))
            .count(),
        2
    );
    assert_eq!(
        prefix
            .events
            .iter()
            .filter(|event| matches!(event.event.fact, Fact::InvocationEnded { .. }))
            .count(),
        1
    );
    for id in ["steer", "next"] {
        assert!(
            log.message_cancelled("message-interrupt", id)
                .await
                .unwrap()
        );
    }
    assert!(
        log.pending_messages("message-interrupt")
            .await
            .unwrap()
            .is_empty()
    );
    log.close().await.unwrap();
}
