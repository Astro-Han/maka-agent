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

use super::support::{client_probe::ClientFixture, peer::Peer};
use maka_client::{Client, Notification};
use maka_event_log::sessions::SessionRetirement;
use maka_fs_tools::worktree::Worktrees;
use maka_protocol::session::{
    SandboxMode, SessionRemoveInput, SessionRemovePreviewInput, SessionRemoveQueryInput,
    SessionRemoveQueryResult, SessionRemoveResult, WorkspaceProjection, WorkspaceTarget,
};
use maka_protocol::subscription::{
    AssistantObservationFrame, ObservationFrame, SubscriptionClosedReason, SubscriptionOpenInput,
    TranscriptPolicy,
};
use maka_runtime_host::{
    server::{Host, HostOperations, local::LocalListener},
    session::{PreparedSession, SessionConfiguration, SessionTarget},
};
use serde_json::json;
use sqlx::Connection;
use std::{
    process::Command,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removal_recovers_without_its_requester_and_releases_only_the_last_workspace_owner() {
    let fixture = ClientFixture::new("maka-removal-");
    let cwd =
        maka_fs_tools::workspace::project::host_path(&fixture.workspace.canonicalize().unwrap())
            .unwrap()
            .to_owned();
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Maka tests"],
        vec!["config", "user.email", "tests@maka.invalid"],
        vec!["commit", "--quiet", "--allow-empty", "-m", "base"],
    ] {
        let output = Command::new("git")
            .current_dir(&cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let root = fixture.owner().canonical_path().to_owned();
    let worktrees = Worktrees::open(&root.join("subagent-worktrees")).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let binding = worktrees
        .plan(std::path::Path::new(&cwd), &"a".repeat(64), cancel.clone())
        .unwrap();
    worktrees.ensure(&binding, &cancel).unwrap();
    let configuration = PreparedSession::new(serde_json::from_value(json!({
        "sessionId":"source", "workspace":{"kind":"host_path","path":cwd}, "executorId":"fixture"
    })).unwrap()).unwrap().bind(
        WorkspaceProjection { target: WorkspaceTarget::HostPath { path: cwd.clone() }, host_cwd: cwd.clone() },
        SessionTarget::Executor { executor_id: "fixture".to_owned().try_into().unwrap(), settings: Default::default() },
        SandboxMode::DangerFullAccess,
    );
    let log = fixture.log().await;
    log.create_session("source", "source", &configuration, 1)
        .await
        .unwrap();
    let mut shared = configuration.clone();
    shared.workspace_origin = maka_runtime::execution::WorkspaceOrigin::Allocated;
    shared.workspace.host_cwd = binding.directory().to_str().unwrap().into();
    shared.workspace.target = WorkspaceTarget::HostPath {
        path: shared.workspace.host_cwd.clone(),
    };
    shared.worktree = Some(binding.clone());
    for session in ["first", "last"] {
        log.create_session(session, session, &shared, 1)
            .await
            .unwrap();
    }
    log.close().await.unwrap();

    for session in ["first", "last"] {
        let log = fixture.log().await;
        log.begin_session_removal(session, 1).await.unwrap();
        log.close().await.unwrap();
        // The accepting process disappeared before scheduling any cleanup.
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("removal.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-removal-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let _cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop.clone()),
        );
        let (mut peer, hello) = Peer::handshake(host.clone(), "removal").await;
        let (client, mut notices) = Client::connect(
            maka_client::local::open_stream(&endpoint).await.unwrap(),
            hello["rootId"].as_str().unwrap(),
            hello["hostEpoch"].as_str().unwrap(),
            HostOperations,
        )
        .await
        .unwrap();
        let mut read = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .filename(root.join(maka_event_log::root::ROOT_DATABASE))
                .read_only(true),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !sqlx::query_scalar::<_, bool>(
                "SELECT completed FROM session_retirements WHERE session_id=?",
            )
            .bind(session)
            .fetch_one(&mut read)
            .await
            .unwrap()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        read.close().await.unwrap();
        assert!(
            matches!(
                client.query_session_removal(SessionRemoveQueryInput {
                    session_id: session.into(),
                }).await.unwrap(),
                SessionRemoveQueryResult::Removed { session_id, archived_subtask_count: 0 }
                    if session_id == session
            ),
            "native Client can reconcile after the accepting process disappeared"
        );
        if session == "first" {
            worktrees.inspect(&binding).unwrap();
            let selected = peer
                .rpc(
                    "session.workspace.relocate",
                    json!({
                        "sessionId":"source", "expectedRevision":1,
                        "workspace":{"kind":"host_path","path":binding.directory()}
                    }),
                )
                .await;
            assert_eq!(
                selected["error"]["code"], "operation_conflict",
                "{selected}"
            );
        } else {
            assert!(!binding.directory().exists());
            assert!(worktrees.ensure(&binding, &cancel).is_err());
        }
        let visible = peer
            .rpc(
                "session.catalog.query",
                json!({"kind":"get","sessionId":"source"}),
            )
            .await;
        assert_eq!(
            visible["ok"], true,
            "unrelated Session remains available: {visible}"
        );
        if session == "last" {
            let opened = client
                .open_subscription(SubscriptionOpenInput {
                    session_id: "source".into(),
                    transcript: TranscriptPolicy::Tail { max_bytes: 1024 },
                })
                .await
                .unwrap();
            client
                .ready_subscription(&opened.subscription_id)
                .await
                .unwrap();
            let missing = client
                .query_session_removal(SessionRemoveQueryInput {
                    session_id: "source".into(),
                })
                .await;
            assert_eq!(missing.unwrap(), SessionRemoveQueryResult::Missing);
            let preview = client
                .preview_session_removal(SessionRemovePreviewInput {
                    session_id: "source".into(),
                })
                .await
                .unwrap();
            assert_eq!(preview.archivable_subtask_count, 0);
            let conflict = client
                .remove_session(SessionRemoveInput {
                    session_id: "source".into(),
                    expected_revision: 2,
                })
                .await
                .unwrap();
            assert_eq!(
                conflict,
                SessionRemoveResult::RevisionConflict {
                    expected_revision: 2,
                    actual_revision: 1,
                }
            );
            assert!(client.session("source").await.unwrap().is_some());
            let input = json!({"sessionId":"source", "expectedRevision":1});
            let removed = client
                .remove_session(SessionRemoveInput {
                    session_id: "source".into(),
                    expected_revision: 1,
                })
                .await
                .unwrap();
            assert!(
                matches!(removed, SessionRemoveResult::Removed { ref session_id, .. } if session_id == "source")
            );
            let replay = peer.rpc("session.remove", input).await;
            assert_eq!(
                replay["result"],
                serde_json::to_value(&removed).unwrap(),
                "{replay}"
            );
            let receipt = client
                .query_session_removal(SessionRemoveQueryInput {
                    session_id: "source".into(),
                })
                .await
                .unwrap();
            assert_eq!(
                receipt,
                SessionRemoveQueryResult::Removed {
                    session_id: "source".into(),
                    archived_subtask_count: 0,
                }
            );
            assert!(client.session("source").await.unwrap().is_none());
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Notification::Observation(frame) = notices.recv().await.unwrap()
                        && let ObservationFrame::Assistant(AssistantObservationFrame::Closed {
                            subscription_id,
                            reason,
                            ..
                        }) = *frame
                    {
                        assert_eq!(subscription_id, opened.subscription_id);
                        assert_eq!(reason, SubscriptionClosedReason::SessionRemoved);
                        break;
                    }
                }
            })
            .await
            .unwrap();
        }
        client.disconnect();
        peer.close().await;
        stop.cancel();
        server.await.unwrap().unwrap();
        drop(host);
        let log = fixture.log().await;
        assert_eq!(
            log.session_retirement(session).await.unwrap(),
            Some(SessionRetirement::Removed)
        );
        assert!(
            log.get_session::<SessionConfiguration>(session)
                .await
                .unwrap()
                .is_none()
        );
        log.close().await.unwrap();
    }
}
