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
use maka_runtime::{event::Fact, tool_call::tool_use_id};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::Value;
use sqlx::{
    Connection,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_forms_preserve_dispatch_answer_and_outcome_boundaries() {
    for disconnected in [false, true] {
        verify_forms(disconnected).await;
    }
}

async fn verify_forms(disconnected: bool) {
    let directory = tempfile::Builder::new()
        .prefix("maka-forms-")
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
                .arg("--form-workspace")
                .arg(client_workspace);
            if reopened {
                command.arg("--reopened");
            }
            if disconnected {
                command.arg("--form-disconnect");
            }
            command.output().unwrap()
        });
        let checkpoints = async {
            if reopened {
                return;
            }
            let checkpoints = if disconnected {
                vec![("pending-5", 1, 1, 0, 0), ("final", 1, 1, 1, 0)]
            } else {
                vec![
                    ("pending-1", 1, 1, 0, 0),
                    ("invalid", 1, 1, 0, 0),
                    ("received-1", 1, 1, 1, 0),
                    ("pending-2", 1, 2, 1, 0),
                    ("received-2", 1, 2, 2, 0),
                    ("settled-1", 1, 2, 2, 1),
                    ("pending-3", 2, 3, 2, 1),
                    ("received-3", 2, 3, 3, 1),
                    ("settled-2", 2, 3, 3, 2),
                    ("pending-4", 3, 4, 3, 2),
                    ("final", 3, 4, 4, 2),
                ]
            };
            for (phase, dispatch_count, request_count, outcome_count, settled_count) in checkpoints
            {
                let marker = workspace.join(phase);
                while !marker.with_extension("json").exists() {
                    if client.is_finished() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                if phase == "final" {
                    loop {
                        let count: i64 =
                            sqlx::query_scalar("SELECT COUNT(*) FROM interaction_outcomes")
                                .fetch_one(&mut reader)
                                .await
                                .unwrap();
                        if count == outcome_count {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    let outcome: String = sqlx::query_scalar(
                        "SELECT outcome_json FROM interaction_outcomes ORDER BY rowid DESC LIMIT 1",
                    )
                    .fetch_one(&mut reader)
                    .await
                    .unwrap();
                    let outcome: Value = serde_json::from_str(&outcome).unwrap();
                    assert_eq!(
                        outcome["reason"],
                        if disconnected {
                            "producer_cancelled"
                        } else {
                            "turn_stopped"
                        }
                    );
                }
                let events: Vec<maka_runtime::event::RuntimeEvent> =
                    sqlx::query_scalar::<_, String>(
                        "SELECT event_json FROM runtime_events ORDER BY sequence",
                    )
                    .fetch_all(&mut reader)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|json| serde_json::from_str(&json).unwrap())
                    .collect();
                let dispatches: Vec<_> = events
                    .iter()
                    .filter(|e| matches!(&e.fact, Fact::ToolDispatched { name, .. } if name == "mcp__forms__collect"))
                    .collect();
                assert_eq!(
                    dispatches.len(),
                    dispatch_count,
                    "{phase}: T1 precedes Form"
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|e| {
                            matches!(e.fact, Fact::ToolSettled { .. })
                                && dispatches.iter().any(|dispatch| {
                                    dispatch.invocation == e.invocation
                                        && dispatch.fact.operation_id() == e.fact.operation_id()
                                })
                        })
                        .count(),
                    settled_count,
                    "{phase}: only provider final result commits T2; cancelled effects stay unknown"
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
                assert_eq!(
                    outcomes, outcome_count,
                    "{phase}: answer durable before provider continues"
                );
                assert_eq!(grants, 0, "Form never grants a capability");
                let evidence: Value =
                    serde_json::from_slice(&std::fs::read(marker.with_extension("json")).unwrap())
                        .unwrap();
                if phase.starts_with("pending") {
                    let pending = &evidence["pending"];
                    let event = dispatches.last().unwrap();
                    let Fact::ToolDispatched { operation_id, .. } = &event.fact else {
                        unreachable!()
                    };
                    assert_eq!(
                        pending["request"]["toolUseId"],
                        tool_use_id(&event.invocation.invocation_id, operation_id)
                    );
                    assert_eq!(pending["turnId"], event.invocation.turn_id);
                    assert_eq!(pending["runId"], event.invocation.run_id);
                }
                if phase.starts_with("received") {
                    let snapshot: Value = serde_json::from_slice(
                        &std::fs::read(
                            workspace.join(format!("pending-{}.json", evidence["ordinal"])),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let canonical: String = sqlx::query_scalar(
                        "SELECT outcome_json FROM interaction_outcomes WHERE request_id = ?",
                    )
                    .bind(snapshot["pending"]["interactionId"].as_str().unwrap())
                    .fetch_one(&mut reader)
                    .await
                    .unwrap();
                    let canonical: Value = serde_json::from_str(&canonical).unwrap();
                    assert_eq!(canonical["action"], evidence["result"]["action"]);
                    assert_eq!(
                        canonical["values"]["count"].as_f64(),
                        evidence["result"]["values"]["count"].as_f64()
                    );
                }
                std::fs::write(marker.with_extension("ok"), b"verified").unwrap();
            }
        };
        let checked = tokio::time::timeout(
            Duration::from_secs(25),
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
        if !output.status.success() {
            eprintln!(
                "checkpoint files: {:?}",
                std::fs::read_dir(&workspace)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect::<Vec<_>>()
            );
        }
        assert!(
            output.status.success(),
            "status: {}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        checked.unwrap().unwrap();
        let mut rows = Vec::new();
        for sql in [
            "SELECT record_json FROM interaction_requests ORDER BY request_id",
            "SELECT outcome_json FROM interaction_outcomes ORDER BY request_id",
            "SELECT event_json FROM runtime_events WHERE json_extract(event_json, '$.invocation.turn_id') IN ('form-1', 'form-2') ORDER BY sequence",
            "SELECT event_json FROM runtime_events WHERE kind IN ('tool_dispatched', 'tool_settled') ORDER BY sequence",
        ] {
            rows.push(
                sqlx::query_scalar::<_, String>(sql)
                    .fetch_all(&mut reader)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(rows[0].len(), if disconnected { 1 } else { 4 });
        assert_eq!(rows[1].len(), if disconnected { 1 } else { 4 });
        // Aborted invocations may gain a recovery terminal; completed facts,
        // all interaction bytes, and effect boundaries must remain identical.
        if let Some(previous) = &canonical {
            assert_eq!(
                &rows, previous,
                "reopen preserves canonical bytes without replay"
            );
        } else {
            canonical = Some(rows);
        }
        reader.close().await.unwrap();
    }
}
