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

use futures_util::FutureExt;
use maka_event_log::{
    EventLog,
    root::{ROOT_DATABASE, RootNamespaces, RootOwner},
};
use maka_runtime::event::{Fact, StoredEvent, ToolOutcome};
use maka_runtime_host::server::{Host, local::LocalListener};
use sha2::{Digest, Sha256};
use sqlx::{
    Connection,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_capabilities_pin_admit_settle_release_and_reopen() {
    #[cfg(unix)]
    let directory = tempfile::Builder::new()
        .prefix("maka-cap-host-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(windows)]
    let directory = tempfile::Builder::new()
        .prefix("maka-cap-host-")
        .tempdir()
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
    let mut original = None;
    for reopened in [false, true] {
        let host = Host::open(RootOwner::open(&root, &ns).unwrap())
            .await
            .unwrap();
        #[cfg(unix)]
        let socket = directory.path().join("h.sock");
        #[cfg(windows)]
        let socket =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-test-{}", uuid::Uuid::new_v4()));
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
                .arg("--capability-host-workspace")
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
            for index in 1..=3 {
                for phase in ["dispatch", "settled"] {
                    let marker = workspace.join(format!("{phase}-{index}"));
                    let json = marker.with_extension("json");
                    while !json.exists() {
                        if client.is_finished() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    // Read through a separately opened store, not live UI or transport frames.
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
                        .filter(|stored| matches!(&stored.event.fact, Fact::ToolDispatched { name, .. } if name.starts_with("mcp__host-")))
                        .collect();
                    let settlements: Vec<_> = events
                        .iter()
                        .filter(|stored| {
                            matches!(stored.event.fact, Fact::ToolSettled { .. })
                                && dispatches.iter().any(|dispatch| {
                                    dispatch.event.invocation == stored.event.invocation
                                        && dispatch.event.fact.operation_id()
                                            == stored.event.fact.operation_id()
                                })
                        })
                        .collect();
                    assert_eq!(dispatches.len(), index);
                    let dispatched = &dispatches[index - 1].event;
                    let Fact::ToolDispatched {
                        operation_id,
                        input,
                        ..
                    } = &dispatched.fact
                    else {
                        unreachable!()
                    };
                    assert_eq!(input["index"], index);
                    let effect = workspace.join(format!("effect-{index}"));
                    if phase == "dispatch" {
                        assert_eq!(settlements.len(), index - 1);
                        assert!(
                            !effect.exists(),
                            "effect must wait for durable ToolDispatched"
                        );
                        let frame: serde_json::Value =
                            serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
                        let invocation = &dispatched.invocation;
                        assert_eq!(frame["source"]["kind"], "agent");
                        assert_eq!(frame["source"]["sessionId"], invocation.session_id);
                        assert_eq!(frame["source"]["turnId"], invocation.turn_id);
                        let tuple = serde_json::to_vec(&[
                            "maka.tool-presentation.v1",
                            &invocation.invocation_id,
                            operation_id,
                        ])
                        .unwrap();
                        assert_eq!(
                            frame["toolCallId"],
                            format!("tool_{:x}", Sha256::digest(tuple))
                        );
                    } else {
                        assert_eq!(settlements.len(), index);
                        let settled = settlements[index - 1];
                        assert!(settled.sequence > dispatches[index - 1].sequence);
                        assert!(matches!(&settled.event.fact, Fact::ToolSettled {
                            operation_id: settled_id, outcome: ToolOutcome::Succeeded { .. }
                        } if settled_id == operation_id));
                        let Fact::ToolSettled {
                            outcome: ToolOutcome::Succeeded { raw: reference, .. },
                            ..
                        } = &settled.event.fact
                        else {
                            unreachable!()
                        };
                        let payload: Vec<u8> = sqlx::query_scalar(
                            "SELECT payload FROM tool_result_payloads WHERE event_id = ?",
                        )
                        .bind(&settled.event.id)
                        .fetch_one(&mut reader)
                        .await
                        .unwrap();
                        let maka_runtime::tool_output::ToolOutput::Mcp(raw) =
                            maka_runtime::tool_output::decode_raw_tool_result(&payload, reference)
                                .unwrap()
                        else {
                            panic!("client MCP result must preserve its canonical interpretation");
                        };
                        let expected: serde_json::Value =
                            serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
                        assert_eq!(
                            serde_json::to_value(raw).unwrap(),
                            expected,
                            "model media budget must not mutate durable raw evidence"
                        );
                        assert_eq!(std::fs::read_to_string(effect).unwrap(), index.to_string());
                    }
                    std::fs::write(marker.with_extension("ok"), b"verified").unwrap();
                }
            }
        };
        let checkpoint_result = tokio::time::timeout(
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
        checkpoint_result.unwrap().unwrap();
        reader.close().await.unwrap();
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(if reopened {
                "original-client-capability-host-reopened"
            } else {
                "original-client-capability-host"
            })
        );
        let log = EventLog::open(&root.join(ROOT_DATABASE)).await.unwrap();
        let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
        for (index, stored) in prefix
            .events
            .iter()
            .filter(|stored| {
                matches!(
                    stored.event.fact,
                    Fact::ToolSettled {
                        outcome: ToolOutcome::Succeeded { .. },
                        ..
                    }
                ) && prefix.events.iter().any(|dispatch| {
                    let Fact::ToolDispatched { name, .. } = &dispatch.event.fact else {
                        return false;
                    };
                    name.starts_with("mcp__host-")
                        && dispatch.event.invocation == stored.event.invocation
                        && dispatch.event.fact.operation_id() == stored.event.fact.operation_id()
                })
            })
            .enumerate()
        {
            let maka_runtime::tool_output::ToolOutput::Mcp(raw) = log
                .resolve_tool_result(&stored.event.invocation.session_id, &stored.event.id)
                .await
                .unwrap()
            else {
                panic!("canonical client result must retain MCP format")
            };
            let expected: serde_json::Value = serde_json::from_slice(
                &std::fs::read(workspace.join(format!("settled-{}.json", index + 1))).unwrap(),
            )
            .unwrap();
            assert_eq!(serde_json::to_value(raw).unwrap(), expected);
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(workspace.join("capability-rows.json")).unwrap())
                .unwrap();
        for stored in &prefix.events {
            if let Fact::ToolDispatched { operation_id, .. } = &stored.event.fact {
                let invocation = &stored.event.invocation;
                let tuple = serde_json::to_vec(&[
                    "maka.tool-presentation.v1",
                    &invocation.invocation_id,
                    operation_id,
                ])
                .unwrap();
                let presentation_id = format!("tool_{:x}", Sha256::digest(tuple));
                assert!(rows.iter().any(|row| row["type"] == "tool_call"
                    && row["id"] == presentation_id
                    && row["turnId"] == invocation.turn_id));
            }
        }
        assert_eq!(
            prefix
                .events
                .iter()
                .filter(|row| matches!(row.event.fact, Fact::ToolSettled { .. }))
                .count(),
            5
        );
        assert!(
            !prefix
                .events
                .iter()
                .any(|row| matches!(row.event.fact, Fact::ToolRejected { .. }))
        );
        let bytes = serde_json::to_vec(&prefix).unwrap();
        if let Some(original) = &original {
            assert_eq!(
                &bytes, original,
                "reopen must preserve exact canonical facts and digest"
            );
        } else {
            original = Some(bytes);
        }
        log.close().await.unwrap();
    }
}
