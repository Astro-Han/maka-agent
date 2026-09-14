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

use maka_event_log::EventLog;
use maka_runtime::{
    shell_run::{ShellOutcome, ShellOutput, ShellPatch, ShellRun, ShellState, ShellVisibility},
    terminal::{
        MouseEncoding, MouseTracking, TerminalCursor, TerminalInputModes, TerminalScreen,
        TerminalSize,
    },
};
use std::path::Path;

pub(crate) async fn seed(log: &EventLog, workspace: &Path) {
    let mut records = Vec::new();
    for index in 0..67 {
        let output = match index {
            1 | 2 => ShellOutput::Pty {
                screen: TerminalScreen {
                    screen: if index == 1 {
                        "界".repeat(40_000)
                    } else {
                        "x".repeat(27_900)
                    },
                    scrollback: "old scrollback\n".repeat(8000),
                    last_alternate_screen: Some("alternate 😀\n".repeat(3000)),
                    size: TerminalSize::new(100, 30).unwrap(),
                    cursor: TerminalCursor {
                        x: 99,
                        y: 29,
                        visible: true,
                    },
                    alternate_screen: true,
                    truncated: false,
                    input: TerminalInputModes {
                        application_cursor_keys_mode: true,
                        mouse_tracking_mode: MouseTracking::Drag,
                        mouse_encoding: MouseEncoding::Sgr,
                    },
                },
            },
            _ => ShellOutput::Pipes {
                stdout: match index {
                    0 => "line\n".repeat(3000),
                    3 => "界".repeat(300_000),
                    4 => "line\n".repeat(2001),
                    _ => format!("output-{index}\n"),
                },
                stderr: if index == 3 {
                    "\0".repeat(20_000)
                } else {
                    String::new()
                },
                latest_stream: None,
                stdout_truncated: false,
                stderr_truncated: false,
            },
        };
        let record = ShellRun {
            id: format!("resource-{index:03}"),
            session_id: "bash-bypass".into(),
            source_run_id: None,
            source_turn_id: "resource-turn".into(),
            source_tool_call_id: if index == 2 {
                "\0".repeat(512)
            } else {
                format!("call-{index}")
            },
            visibility: if index % 2 == 0 {
                ShellVisibility::User
            } else {
                ShellVisibility::Model
            },
            cwd: workspace.to_string_lossy().into_owned(),
            command: if index == 2 {
                "c".repeat(20_700)
            } else {
                "never replay this command".into()
            },
            started_at: 10,
            updated_at: 10,
            timeout_ms: None,
            revision: 1,
            state: ShellState::Starting,
            output,
        };
        log.create_shell_run(record.clone()).await.unwrap();
        log.patch_shell_run(
            "bash-bypass",
            &record.id,
            ShellPatch {
                state: Some(ShellState::Running),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let record = log
            .patch_shell_run(
                "bash-bypass",
                &record.id,
                ShellPatch {
                    state: Some(ShellState::Terminal {
                        completed_at: 20,
                        outcome: ShellOutcome::Completed,
                        observed_at: None,
                    }),
                    updated_at: Some(20),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        records.push(record);
    }
    let first = log
        .query_shell_resources("bash-bypass", None, 0)
        .await
        .unwrap();
    let original = first.revision;
    log.patch_shell_run(
        "bash-bypass",
        "resource-066",
        ShellPatch {
            output: Some(ShellOutput::Pipes {
                stdout: "changed tail".into(),
                stderr: String::new(),
                latest_stream: None,
                stdout_truncated: false,
                stderr_truncated: false,
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_ne!(
        original,
        log.query_shell_resources("bash-bypass", None, 0)
            .await
            .unwrap()
            .revision
    );
    // Restore output; revision remains advanced and is itself a visible fact.
    let last = records.last_mut().unwrap();
    *last = log
        .patch_shell_run(
            "bash-bypass",
            &last.id,
            ShellPatch {
                output: Some(last.output.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    std::fs::write(
        workspace.join("resource-records.json"),
        serde_json::to_vec(&records).unwrap(),
    )
    .unwrap();
}
