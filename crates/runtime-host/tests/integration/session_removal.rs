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
use maka_event_log::sessions::SessionRetirement;
use maka_fs_tools::worktree::Worktrees;
use maka_protocol::session::{SandboxMode, WorkspaceProjection, WorkspaceTarget};
use maka_runtime_host::{
    server::{Host, local::LocalListener},
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
        let mut peer = Peer::new(host.clone(), "removal").await;
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
            let missing = peer
                .rpc("session.remove.query", json!({"sessionId":"source"}))
                .await;
            assert_eq!(missing["result"]["kind"], "missing", "{missing}");
            let preview = peer
                .rpc("session.remove.preview", json!({"sessionId":"source"}))
                .await;
            assert_eq!(preview["result"]["archivableSubtaskCount"], 0, "{preview}");
            let input = json!({"sessionId":"source", "expectedRevision":1});
            let removed = peer.rpc("session.remove", input.clone()).await;
            assert_eq!(removed["result"]["kind"], "removed", "{removed}");
            let replay = peer.rpc("session.remove", input).await;
            assert_eq!(replay["result"], removed["result"], "{replay}");
            let receipt = peer
                .rpc("session.remove.query", json!({"sessionId":"source"}))
                .await;
            assert_eq!(receipt["result"], removed["result"], "{receipt}");
        }
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
