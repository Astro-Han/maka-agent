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
use maka_runtime::{execution::ToolMode, workhub::COORDINATION_SESSION_ID};
use maka_runtime_host::session::SessionConfiguration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_workhub_existing_and_created_tasks_run_once_and_replay_after_reopen() {
    for flag in [
        "--workhub-delegation-workspace",
        "--workhub-creation-workspace",
    ] {
        let fixture = ClientFixture::new("maka-workhub-delegation-");
        fixture.run(flag, false, "workhub-delegation-passed").await;
        let log = fixture.log().await;
        let before = log.prefix(256, 512 * 1024).await.unwrap();
        use maka_runtime::event::Fact;
        assert_eq!(
            before
                .events
                .iter()
                .filter(|row| matches!(row.event.fact, Fact::WorkhubDelegated { .. }))
                .count(),
            1
        );
        assert_eq!(
            before
                .events
                .iter()
                .filter(|row| matches!(row.event.fact, Fact::InvocationOpened { .. }))
                .count(),
            2
        );
        let delegation = before
            .events
            .iter()
            .find_map(|row| match &row.event.fact {
                Fact::WorkhubDelegated { delegation } => Some(delegation),
                _ => None,
            })
            .unwrap();
        assert!(
            log.pending_messages(&delegation.target.session_id)
                .await
                .unwrap()
                .is_empty()
        );
        log.close().await.unwrap();
        fixture.run(flag, true, "workhub-delegation-reopened").await;
        let log = fixture.log().await;
        assert_eq!(
            serde_json::to_value(log.prefix(256, 512 * 1024).await.unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        log.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_workhub_answer_scopes_read_and_desktop_calls_without_replaying_effects() {
    let fixture = ClientFixture::new("maka-workhub-answer-");
    fixture
        .run("--workhub-answer-workspace", false, "workhub-answer-passed")
        .await;
    let log = fixture.log().await;
    let before = log.prefix(256, 512 * 1024).await.unwrap();
    use maka_runtime::event::Fact;
    assert_eq!(
        before
            .events
            .iter()
            .filter(|row| matches!(row.event.fact, Fact::InvocationOpened { .. }))
            .count(),
        1
    );
    assert!(
        before
            .events
            .iter()
            .any(|row| matches!(row.event.fact, Fact::ToolDispatched { .. }))
    );
    log.close().await.unwrap();
    fixture
        .run(
            "--workhub-answer-workspace",
            true,
            "workhub-answer-reopened",
        )
        .await;
    let log = fixture.log().await;
    assert_eq!(
        serde_json::to_value(log.prefix(256, 512 * 1024).await.unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    log.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_workhub_identity_model_cas_and_readonly_query_survive_reopen() {
    let fixture = ClientFixture::new("maka-workhub-client-");
    fixture
        .run("--workhub-workspace", false, "workhub-passed")
        .await;
    let log = fixture.log().await;
    let before = log
        .get_session::<SessionConfiguration>(COORDINATION_SESSION_ID)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before.configuration.tool_profile,
        Some(maka_protocol::session::SessionToolProfile::WorkhubCoordinationV2)
    );
    assert_eq!(before.configuration.tool_mode, ToolMode::Direct);
    assert!(!before.archived);
    assert!(
        log.prefix(8, 4096).await.unwrap().events.is_empty(),
        "WorkHub control state is not fabricated execution history"
    );
    log.close().await.unwrap();
    fixture
        .run("--workhub-workspace", true, "workhub-reopened")
        .await;
    let log = fixture.log().await;
    assert_eq!(
        log.get_session::<SessionConfiguration>(COORDINATION_SESSION_ID)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert!(log.prefix(8, 4096).await.unwrap().events.is_empty());
    log.close().await.unwrap();
}
