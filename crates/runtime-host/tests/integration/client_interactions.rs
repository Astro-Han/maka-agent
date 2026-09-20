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

#![cfg(unix)]
use futures_util::FutureExt;
use maka_event_log::root::{ROOT_DATABASE, RootNamespaces, RootOwner};
use maka_runtime::event::{Fact, StoredEvent};
use maka_runtime_host::server::{Host, local::LocalListener};
use sqlx::{
    Connection,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_managed_approval_gates_effects_and_persists_decisions() {
    let directory = tempfile::Builder::new()
        .prefix("maka-approval-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let ns = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let root = directory.path().join("root");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let owner = RootOwner::create(&root, &ns).unwrap();
    let root_id = owner.root_id().to_owned();
    drop(owner);
    let mut canonical = None;
    for reopened in [false, true] {
        let host = Host::open(RootOwner::open(&root, &ns).unwrap())
            .await
            .unwrap();
        let socket = directory.path().join("h.sock");
        let listener = LocalListener::bind(&socket).unwrap();
        let cancellation = CancellationToken::new();
        let server = tokio::spawn(listener.serve(host, cancellation.clone()));
        let mut reader = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(root.join(ROOT_DATABASE))
                .read_only(true),
        )
        .await
        .unwrap();
        let client_workspace = workspace.clone();
        let expected_id = root_id.clone();
        let client = tokio::task::spawn_blocking(move || {
            let probe =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs");
            let mut command = Command::new("node");
            command
                .arg(probe)
                .arg("--socket")
                .arg(socket)
                .args(["--root-id", &expected_id])
                .arg("--managed-approval-workspace")
                .arg(client_workspace);
            if reopened {
                command.arg("--reopened");
            }
            command.output().unwrap()
        });
        let checkpoints = async {
            if reopened {
                return;
            }
            for (phase, dispatch_count, request_count, outcome_count) in [
                ("pending-1", 0, 1, 0),
                ("dispatch-1", 1, 1, 1),
                ("dispatch-2", 2, 1, 1),
                ("pending-3", 2, 2, 1),
                ("pending-4", 2, 3, 2),
                ("pending-5", 2, 4, 3),
                ("final", 2, 4, 4),
            ] {
                let marker = workspace.join(phase);
                while !marker.with_extension("json").exists() {
                    if client.is_finished() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let events: Vec<StoredEvent> = sqlx::query_as::<_, (i64, String)>(
                    "SELECT sequence, event_json FROM runtime_events ORDER BY sequence",
                )
                .fetch_all(&mut reader)
                .await
                .unwrap()
                .into_iter()
                .map(|(sequence, json)| StoredEvent {
                    sequence: sequence.try_into().unwrap(),
                    event: serde_json::from_str(&json).unwrap(),
                })
                .collect();
                let dispatches: Vec<_> = events
                    .iter()
                    .filter(|e| matches!(&e.event.fact, Fact::ToolDispatched { name, .. } if name == "mcp__desktop_browser__browser_navigate"))
                    .collect();
                assert_eq!(
                    dispatches.len(),
                    dispatch_count,
                    "{phase}: no dispatch before canonical approval"
                );
                let requests: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM interaction_requests")
                    .fetch_one(&mut reader)
                    .await
                    .unwrap();
                let outcomes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM interaction_outcomes")
                    .fetch_one(&mut reader)
                    .await
                    .unwrap();
                let grants: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM client_capability_session_grants")
                        .fetch_one(&mut reader)
                        .await
                        .unwrap();
                assert_eq!(requests, request_count, "{phase}");
                assert_eq!(outcomes, outcome_count, "{phase}");
                assert_eq!(
                    grants,
                    if outcome_count > 0 { 1 } else { 0 },
                    "{phase}: only allow may grant"
                );
                if phase.starts_with("dispatch") {
                    let index = dispatch_count;
                    assert!(!workspace.join(format!("effect-{index}")).exists());
                    let frame: serde_json::Value = serde_json::from_slice(
                        &std::fs::read(marker.with_extension("json")).unwrap(),
                    )
                    .unwrap();
                    let stored = dispatches.last().unwrap();
                    assert_eq!(frame["source"]["kind"], "agent");
                    assert_eq!(frame["source"]["turnId"], stored.event.invocation.turn_id);
                    assert_eq!(stored.event.invocation.turn_id, format!("approval-{index}"));
                    let Fact::ToolDispatched { input, .. } = &stored.event.fact else {
                        unreachable!()
                    };
                    assert_eq!(input["index"], index);
                }
                if phase == "final" {
                    assert_eq!(
                        events
                            .iter()
                            .filter(|e| {
                                matches!(e.event.fact, Fact::ToolSettled { .. })
                                    && dispatches.iter().any(|dispatch| {
                                        dispatch.event.invocation == e.event.invocation
                                            && dispatch.event.fact.operation_id()
                                                == e.event.fact.operation_id()
                                    })
                            })
                            .count(),
                        2
                    );
                    for index in 1..=2 {
                        assert_eq!(
                            std::fs::read_to_string(workspace.join(format!("effect-{index}")))
                                .unwrap(),
                            index.to_string()
                        );
                    }
                    for index in 3..=5 {
                        assert!(!workspace.join(format!("effect-{index}")).exists());
                    }
                }
                std::fs::write(marker.with_extension("ok"), b"verified").unwrap();
            }
        };
        let checked = tokio::time::timeout(
            Duration::from_secs(20),
            std::panic::AssertUnwindSafe(checkpoints).catch_unwind(),
        )
        .await;
        let output = tokio::time::timeout(Duration::from_secs(20), client).await;
        cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let output = output.unwrap().unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        checked.unwrap().unwrap();
        let mut rows = Vec::new();
        for sql in [
            "SELECT record_json FROM interaction_requests ORDER BY request_id",
            "SELECT outcome_json FROM interaction_outcomes ORDER BY request_id",
            "SELECT record_json FROM client_capability_session_grants ORDER BY authority_key",
            "SELECT event_json FROM runtime_events ORDER BY sequence",
        ] {
            rows.push(
                sqlx::query_scalar::<_, String>(sql)
                    .fetch_all(&mut reader)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(rows[0].len(), 4);
        assert_eq!(rows[1].len(), 4);
        assert_eq!(rows[2].len(), 1);
        if let Some(previous) = &canonical {
            assert_eq!(
                &rows, previous,
                "reopen preserves canonical outcomes/grant/events"
            );
        } else {
            canonical = Some(rows);
        }
        reader.close().await.unwrap();
    }
}
