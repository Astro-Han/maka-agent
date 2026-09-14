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
use maka_runtime::shell_run::{
    MAX_SHELL_RECORD_BYTES, ShellOutcome, ShellOutput, ShellPatch, ShellRun, ShellState,
    ShellVisibility,
};
use serde_json::json;

#[tokio::test]
async fn admitted_output_leaves_room_for_recovery_and_first_observation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let mut record = ShellRun {
        id: "large".into(),
        session_id: "session".into(),
        source_run_id: None,
        source_turn_id: "turn".into(),
        source_tool_call_id: "call".into(),
        visibility: ShellVisibility::User,
        cwd: "/workspace".into(),
        command: "command".into(),
        started_at: 1,
        updated_at: 1,
        timeout_ms: None,
        revision: 1,
        state: ShellState::Starting,
        output: output(0),
    };
    let empty_bytes = serde_json::to_vec(&record).unwrap().len();
    record.output = output(MAX_SHELL_RECORD_BYTES - empty_bytes);
    assert!(
        log.create_shell_run(record.clone()).await.is_err(),
        "full active payload must be rejected before any process could spawn"
    );
    record.output = output(MAX_SHELL_RECORD_BYTES - empty_bytes - 512);
    log.create_shell_run(record.clone()).await.unwrap();
    log.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    let latest = 9_007_199_254_740_991;
    assert_eq!(log.recover_shell_runs(latest).await.unwrap(), 1);
    let orphan = log
        .read_shell_run("session", "large")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        orphan.output, record.output,
        "recovery must preserve the admitted output"
    );
    assert!(matches!(
        orphan.state,
        ShellState::Terminal {
            outcome: ShellOutcome::Orphaned { .. },
            ..
        }
    ));
    let mut empty = orphan.clone();
    empty.output = output(0);
    let empty_bytes = serde_json::to_vec(&empty).unwrap().len();
    assert!(
        log.patch_shell_run(
            "session",
            "large",
            ShellPatch {
                output: Some(output(MAX_SHELL_RECORD_BYTES - empty_bytes)),
                ..Default::default()
            }
        )
        .await
        .is_err(),
        "first observation still needs space after a terminal output flush"
    );
    log.patch_shell_run(
        "session",
        "large",
        ShellPatch {
            output: Some(output(MAX_SHELL_RECORD_BYTES - empty_bytes - 32)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let observed = log
        .patch_shell_run(
            "session",
            "large",
            ShellPatch {
                observed_at: Some(latest),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(observed.state, ShellState::Terminal {
        observed_at: Some(at), .. } if at == latest));
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.read_shell_run("session", "large").await.unwrap(),
        Some(observed)
    );
    assert_eq!(log.recover_shell_runs(latest).await.unwrap(), 0);
    log.close().await.unwrap();
}

fn output(bytes: usize) -> ShellOutput {
    ShellOutput::Pipes {
        stdout: "x".repeat(bytes),
        stderr: String::new(),
        latest_stream: None,
        stdout_truncated: false,
        stderr_truncated: false,
    }
}
