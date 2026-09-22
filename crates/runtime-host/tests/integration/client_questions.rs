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
use maka_runtime::{
    event::{Fact, RuntimeEvent, ToolOutcome},
    tool_call::tool_use_id,
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::Value;
use sqlx::{
    Connection,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_questions_commit_answers_and_stop_without_draining() {
    for mode in ["read-only", "danger-full-access"] {
        verify_questions(mode).await;
    }
}

async fn verify_questions(mode: &'static str) {
    let directory = tempfile::Builder::new()
        .prefix("maka-questions-")
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
                .arg("--question-workspace")
                .arg(client_workspace)
                .args(["--question-mode", mode]);
            if reopened {
                command.arg("--reopened");
            }
            command.output().unwrap()
        });
        let checkpoints = async {
            if reopened {
                return;
            }
            for (phase, dispatch_count, outcomes, settled_count) in [
                ("pending-1", 1, 0, 0),
                ("invalid", 1, 0, 0),
                ("model-result", 1, 1, 1),
                ("pending-2", 2, 1, 1),
                ("stopped", 2, 2, 2),
                ("final", 2, 2, 2),
            ] {
                let marker = workspace.join(phase);
                while !marker.with_extension("json").exists() {
                    if client.is_finished() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let events: Vec<RuntimeEvent> = sqlx::query_scalar::<_, String>(
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
                    .filter(|e| matches!(e.fact, Fact::ToolDispatched { .. }))
                    .collect();
                let settled: Vec<_> = events
                    .iter()
                    .filter(|e| matches!(e.fact, Fact::ToolSettled { .. }))
                    .collect();
                assert_eq!(
                    dispatches.len(),
                    dispatch_count,
                    "{mode}/{phase}: T1 before publication"
                );
                assert_eq!(
                    settled.len(),
                    settled_count,
                    "{mode}/{phase}: unique known T2"
                );
                for (sql, count) in [
                    (
                        "SELECT COUNT(*) FROM interaction_requests",
                        dispatch_count as i64,
                    ),
                    ("SELECT COUNT(*) FROM interaction_outcomes", outcomes),
                    ("SELECT COUNT(*) FROM client_capability_session_grants", 0),
                ] {
                    let actual: i64 = sqlx::query_scalar(sql)
                        .fetch_one(&mut reader)
                        .await
                        .unwrap();
                    assert_eq!(
                        actual, count,
                        "{mode}/{phase}: durable interaction boundary or grant"
                    );
                }
                let evidence: Value =
                    serde_json::from_slice(&std::fs::read(marker.with_extension("json")).unwrap())
                        .unwrap();
                if phase.starts_with("pending") {
                    let pending = &evidence["pending"];
                    let event = dispatches.last().unwrap();
                    let Fact::ToolDispatched {
                        operation_id,
                        name,
                        input,
                        ..
                    } = &event.fact
                    else {
                        unreachable!()
                    };
                    assert_eq!(name, "AskUserQuestion");
                    assert_eq!(input["questions"][0]["question"], "Pick\u{0001}one");
                    assert_eq!(
                        pending["request"]["toolUseId"],
                        tool_use_id(&event.invocation.invocation_id, operation_id)
                    );
                    assert_eq!(pending["turnId"], event.invocation.turn_id);
                    assert_eq!(pending["runId"], event.invocation.run_id);
                }
                if phase == "model-result" {
                    let outcome: String =
                        sqlx::query_scalar("SELECT outcome_json FROM interaction_outcomes")
                            .fetch_one(&mut reader)
                            .await
                            .unwrap();
                    let outcome: Value = serde_json::from_str(&outcome).unwrap();
                    assert_eq!(outcome["kind"], "question_answer");
                    assert_eq!(
                        outcome["answers"],
                        serde_json::json!(["Beta", "a free answer outside the labels", null])
                    );
                    assert!(matches!(
                        settled[0].fact,
                        Fact::ToolSettled {
                            outcome: ToolOutcome::Succeeded { .. },
                            ..
                        }
                    ));
                    let t2 = events
                        .iter()
                        .position(|e| matches!(e.fact, Fact::ToolSettled { .. }))
                        .unwrap();
                    let next_model = events
                        .iter()
                        .rposition(|e| matches!(e.fact, Fact::ModelRequested { .. }))
                        .unwrap();
                    assert!(t2 < next_model, "T2 durable before next model request");
                }
                if phase == "stopped" {
                    assert!(matches!(
                        settled.last().unwrap().fact,
                        Fact::ToolSettled {
                            outcome: ToolOutcome::Failed { .. },
                            ..
                        }
                    ));
                    let turn: Vec<_> = events
                        .iter()
                        .filter(|e| e.invocation.turn_id == "question-2")
                        .collect();
                    let t2 = turn
                        .iter()
                        .position(|e| matches!(e.fact, Fact::ToolSettled { .. }))
                        .unwrap();
                    let seal = turn
                        .iter()
                        .position(|e| matches!(e.fact, Fact::InvocationEnded { .. }))
                        .unwrap();
                    assert!(
                        t2 < seal,
                        "stopped question settles known failure before seal"
                    );
                    assert_eq!(
                        turn.iter()
                            .filter(|e| matches!(e.fact, Fact::ModelRequested { .. }))
                            .count(),
                        1
                    );
                    let outcome: String = sqlx::query_scalar(
                        "SELECT outcome_json FROM interaction_outcomes ORDER BY rowid DESC LIMIT 1",
                    )
                    .fetch_one(&mut reader)
                    .await
                    .unwrap();
                    assert_eq!(
                        serde_json::from_str::<Value>(&outcome).unwrap()["reason"],
                        "turn_stopped"
                    );
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
            "{mode}: stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        checked.unwrap().unwrap();
        let mut rows = Vec::new();
        for sql in [
            "SELECT record_json FROM interaction_requests ORDER BY request_id",
            "SELECT outcome_json FROM interaction_outcomes ORDER BY request_id",
            "SELECT event_json FROM runtime_events ORDER BY sequence",
        ] {
            rows.push(
                sqlx::query_scalar::<_, String>(sql)
                    .fetch_all(&mut reader)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[1].len(), 2);
        if let Some(previous) = &canonical {
            assert_eq!(
                &rows, previous,
                "reopen preserves all canonical bytes and never reasks"
            );
        } else {
            canonical = Some(rows);
        }
        reader.close().await.unwrap();
    }
}
