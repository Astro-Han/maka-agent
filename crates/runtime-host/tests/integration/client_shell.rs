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
use maka_runtime::event::Fact;
use maka_runtime::shell_run::{ShellOutcome, ShellOutput, ShellRun, ShellState, ShellVisibility};
use sha2::{Digest, Sha256};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_runs_shell_and_reopens_without_repeating_process() {
    let fixture = ClientFixture::new("maka-shell-");
    let workspace = &fixture.workspace;
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--shell-workspace",
                reopened,
                if reopened {
                    "original-client-shell-reopened"
                } else {
                    "original-client-shell"
                },
            )
            .await;
        let log = fixture.log().await;
        let prefix = log.prefix(1000, 4 * 1024 * 1024).await.unwrap();
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(workspace.join("shell-rows.json")).unwrap())
                .unwrap();
        let mut dispatched = 0;
        let mut settled = 0;
        let mut rejected = 0;
        let live: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(workspace.join("shell-live.json")).unwrap())
                .unwrap();
        for stored in &prefix.events {
            if !["shell-bypass", "shell-readonly"]
                .contains(&stored.event.invocation.session_id.as_str())
            {
                continue;
            }
            match &stored.event.fact {
                Fact::ToolSettled { .. } => settled += 1,
                Fact::ToolRejected { name, .. } => {
                    rejected += 1;
                    assert_eq!(name, "Shell");
                    assert_eq!(stored.event.invocation.session_id, "shell-readonly");
                }
                _ => {}
            }
            if let Fact::ToolDispatched { operation_id, .. } = &stored.event.fact {
                dispatched += 1;
                let invocation = &stored.event.invocation;
                let tuple = serde_json::to_vec(&[
                    "maka.tool-presentation.v1",
                    &invocation.invocation_id,
                    operation_id,
                ])
                .unwrap();
                let expected = format!("tool_{:x}", Sha256::digest(tuple));
                let start = live
                    .iter()
                    .find(|event| event["type"] == "tool_start" && event["toolUseId"] == expected)
                    .unwrap();
                assert_eq!(
                    start["ts"].as_u64().unwrap(),
                    stored
                        .event
                        .recorded_at
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64
                );
                assert!(rows.iter().any(|row| row["type"] == "tool_call"
                    && row["turnId"] == invocation.turn_id
                    && row["id"] == expected));
            }
        }
        assert_eq!((dispatched, settled, rejected), (2, 2, 0));
        for session in ["model-shell", "model-shell-pty"] {
            let saved: serde_json::Value = serde_json::from_slice(
                &std::fs::read(workspace.join(format!("{session}.json"))).unwrap(),
            )
            .unwrap();
            let id = saved["ref"]
                .as_str()
                .unwrap()
                .strip_prefix("maka://runtime/background-tasks/")
                .unwrap();
            let background = log.read_shell_run(session, id).await.unwrap().unwrap();
            assert_eq!(background.visibility, ShellVisibility::Model);
            assert!(matches!(background.state, ShellState::Terminal {
            observed_at: Some(_), outcome: ShellOutcome::Exited { code, .. }, ..
        } if code.get() == 19));
            let dispatch = prefix.events.iter().find(|stored| {
            stored.event.invocation.session_id == session &&
            stored.event.invocation.turn_id == "background-launch" &&
            matches!(&stored.event.fact, Fact::ToolDispatched { name, .. } if name == "Shell")
        }).unwrap();
            assert_eq!(
                background.source_run_id.as_deref(),
                Some(dispatch.event.invocation.run_id.as_str())
            );
            assert_eq!(background.source_turn_id, dispatch.event.invocation.turn_id);
        }
        let bytes = serde_json::to_vec(&prefix).unwrap();
        if let Some(original) = &original {
            assert_eq!(
                &bytes, original,
                "reopen must preserve canonical facts and raw-byte digest"
            );
        } else {
            original = Some(bytes);
        }
        if !reopened {
            super::support::shell_resources::seed(&log, workspace).await;
            // A durable resource admission survived, but no live handle did.
            // The next Host startup must orphan it without interpreting its command.
            log.create_shell_run(ShellRun {
                id: "abandoned-shell".into(),
                session_id: "shell-bypass".into(),
                source_run_id: None,
                source_turn_id: "abandoned-turn".into(),
                source_tool_call_id: "abandoned-call".into(),
                visibility: ShellVisibility::Model,
                permissions: maka_runtime::shell_run::ShellPermissions {
                    boundary_revision: 0,
                    sandbox: maka_runtime::shell_run::Sandbox::Disabled,
                },
                cwd: workspace.to_string_lossy().into_owned(),
                command: "echo unexpectedly-replayed > shell-replay.txt".into(),
                started_at: 1,
                updated_at: 1,
                timeout_ms: None,
                revision: 1,
                state: ShellState::Starting,
                output: ShellOutput::Pipes {
                    stdout: "last durable output".into(),
                    stderr: String::new(),
                    latest_stream: None,
                    stdout_truncated: false,
                    stderr_truncated: false,
                },
            })
            .await
            .unwrap();
        } else {
            let abandoned = log
                .read_shell_run("shell-bypass", "abandoned-shell")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(abandoned.revision, 2);
            assert!(matches!(
                abandoned.state,
                ShellState::Terminal {
                    outcome: ShellOutcome::Orphaned { .. },
                    ..
                }
            ));
            assert!(matches!(abandoned.output, ShellOutput::Pipes { stdout, .. }
                if stdout == "last durable output"));
            assert!(!workspace.join("shell-replay.txt").exists());
        }
        log.close().await.unwrap();
    }
}
