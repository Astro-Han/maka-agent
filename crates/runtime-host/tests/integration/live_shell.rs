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
use maka_process::pty::PtyCommand;
#[cfg(unix)]
use maka_runtime::terminal::input::InputAction;
use maka_runtime::{
    shell_run::{ShellOutcome, ShellOutput, ShellRun, ShellState, ShellVisibility},
    terminal::{
        MouseEncoding, MouseTracking, TerminalCursor, TerminalInputModes, TerminalScreen,
        TerminalSize,
    },
};
use maka_runtime_host::shell::{ShellHandle, ShellResources};
use serde_json::json;
use sqlx::{Connection, SqliteConnection};
use std::{path::Path, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

use super::live_pipes::launch;

pub(super) fn record(cwd: &Path, id: &str, command: &str) -> ShellRun {
    ShellRun {
        id: id.into(),
        session_id: "session".into(),
        source_run_id: None,
        source_turn_id: "turn".into(),
        source_tool_call_id: id.into(),
        visibility: ShellVisibility::Model,
        cwd: cwd.to_str().unwrap().into(),
        command: command.into(),
        started_at: 1,
        updated_at: 1,
        timeout_ms: Some(10_000),
        revision: 1,
        state: ShellState::Starting,
        output: ShellOutput::Pty {
            screen: TerminalScreen {
                screen: String::new(),
                scrollback: String::new(),
                last_alternate_screen: None,
                size: TerminalSize::new(80, 24).unwrap(),
                cursor: TerminalCursor {
                    x: 0,
                    y: 0,
                    visible: true,
                },
                alternate_screen: false,
                truncated: false,
                input: TerminalInputModes {
                    application_cursor_keys_mode: false,
                    mouse_tracking_mode: MouseTracking::None,
                    mouse_encoding: MouseEncoding::Default,
                },
            },
        },
    }
}
pub(super) fn command(cwd: &Path, unix: &str, windows: &str) -> PtyCommand {
    #[cfg(unix)]
    let mut plan = {
        let _ = windows;
        let mut plan = PtyCommand::new("/bin/sh", cwd);
        plan.args(["-c", unix]);
        plan
    };
    #[cfg(windows)]
    let mut plan = {
        let _ = unix;
        let system = std::env::var_os("SystemRoot").unwrap();
        let mut plan = PtyCommand::new(
            Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            cwd,
        );
        plan.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            windows,
        ]);
        plan
    };
    plan.env("TERM", "xterm-256color");
    plan
}
fn screen(record: &ShellRun) -> &TerminalScreen {
    let ShellOutput::Pty { screen } = &record.output else {
        panic!("not a PTY")
    };
    screen
}
pub(super) async fn wait_text(handle: &mut ShellHandle, text: &str) {
    loop {
        let record = handle.ready().await.unwrap();
        if screen(&record).screen.contains(text) {
            return;
        }
        assert!(record.state.active(), "{record:?}");
        handle.changed().await.unwrap();
    }
}
pub(super) async fn setup(path: &Path) -> Arc<EventLog> {
    let log = Arc::new(EventLog::open(path).await.unwrap());
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    log
}

#[cfg(unix)]
#[tokio::test]
async fn managed_pty_commits_screen_resize_and_mode_aware_input_through_final_drain() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime.sqlite");
        let log = setup(&path).await;
        let drain = CancellationToken::new();
        let resources = ShellResources::new(log.clone(), drain.clone());
        let script = r#"
            stty raw -echo
            printf '\033[6n'
            reply=$(dd bs=1 count=6 2>/dev/null | od -An -tx1 | tr -d ' \n')
            test "$reply" = 1b5b313b3152 || exit 3
            printf '\033[?1h\033[?1003h\033[?1006hready'
            input=$(dd bs=1 count=17 2>/dev/null | od -An -tx1 | tr -d ' \n')
            test "$input" = 1b5b3c303b39313b314d1b4f41e4b8ad0d || exit 4
            test "$(stty size)" = '30 100' || exit 5
            i=0; while test "$i" -lt 4096; do printf 'draining %s\r\n' "$i"; i=$((i+1)); done
            printf 'final-中'
            exit 42
        "#;
        let ticket = resources.start_pty(record(temp.path(), "native", script),
            command(temp.path(), script, ""), TerminalSize::new(80,24).unwrap()).unwrap();
        drop(ticket); // Losing the start waiter cannot detach or cancel its process.
        let mut handle = resources.get("session", "native").unwrap();
        wait_text(&mut handle, "ready").await;
        let durable = log.read_shell_run("session", "native").await.unwrap().unwrap();
        assert!(screen(&durable).input.application_cursor_keys_mode);
        let actions = [
            json!({"type":"mouse","event":"press","button":"left","x":90,"y":0}),
            json!({"type":"key","key":"arrow_up"}),
            json!({"type":"text","text":"中"}),
            json!({"type":"key","key":"enter"}),
        ].into_iter().map(InputAction::parse).collect::<Result<Vec<_>,_>>().unwrap();
        let size = TerminalSize::new(100,30).unwrap();
        let receipt = handle.input_and_resize(actions, Some(size)).await.unwrap();
        assert_eq!(receipt.accepted_bytes, 17);
        assert!(receipt.resized && receipt.resize_changed);
        assert_eq!(screen(&receipt.record).size, size);
        assert_eq!(screen(&log.read_shell_run("session", "native").await.unwrap().unwrap()).size, size);
        let final_record = handle.finished().await.unwrap();
        assert!(matches!(&final_record.state, ShellState::Terminal { outcome: ShellOutcome::Exited { code, .. }, .. } if code.get() == 42));
        assert!(screen(&final_record).screen.ends_with("final-中"), "{final_record:?}");
        assert_eq!(*final_record, log.read_shell_run("session","native").await.unwrap().unwrap());
        resources.shutdown().await;
        assert_eq!(resources.active_count(), 0);
        assert!(!drain.is_cancelled());
        log.shutdown().await.unwrap();
        drop(resources);
        drop(log);
        let reopened = EventLog::open(&path).await.unwrap();
        assert_eq!(reopened.recover_shell_runs(100).await.unwrap(), 0);
        assert_eq!(*final_record, reopened.read_shell_run("session","native").await.unwrap().unwrap());
        reopened.shutdown().await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn managed_shell_sql_fences_prevent_unrecorded_spawn_and_recover_unknown_finalization() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for pty in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime.sqlite");
        let log = setup(&path).await;
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&path);
        let mut faults = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_start BEFORE INSERT ON shell_runs WHEN NEW.id = 't1' BEGIN SELECT RAISE(ABORT, 'T1 fault'); END")
            .execute(&mut faults).await.unwrap();
        let drain = CancellationToken::new();
        let resources = ShellResources::new(log.clone(), drain.clone());
        let mut t1 = launch(&resources, temp.path(), "t1", pty,
            "echo effect >> effect.txt", "'effect' | Add-Content effect.txt");
        assert!(t1.finished().await.is_err());
        assert!(!temp.path().join("effect.txt").exists());
        assert!(log.read_shell_run("session", "t1").await.unwrap().is_none());
        assert!(!drain.is_cancelled());
        sqlx::query("CREATE TRIGGER fail_final BEFORE UPDATE ON shell_runs WHEN NEW.active = 0 BEGIN SELECT RAISE(ABORT, 'T2 fault'); END")
            .execute(&mut faults).await.unwrap();
        let mut t2 = launch(&resources, temp.path(), "t2", pty,
            "echo effect >> effect.txt; printf durable-output", "'effect' | Add-Content effect.txt; Write-Output 'durable-output'");
        assert!(t2.finished().await.is_err());
        assert!(drain.is_cancelled());
        resources.shutdown().await;
        assert_eq!(resources.active_count(), 0);
        let effect = std::fs::read(temp.path().join("effect.txt")).unwrap();
        assert!(!effect.is_empty());
        assert!(log.read_shell_run("session","t2").await.unwrap().unwrap().state.active());
        sqlx::query("DROP TRIGGER fail_final").execute(&mut faults).await.unwrap();
        faults.close().await.unwrap();
        log.shutdown().await.unwrap();
        drop(resources);
        drop(log);
        let reopened = EventLog::open(&path).await.unwrap();
        assert_eq!(reopened.recover_shell_runs(100).await.unwrap(), 1);
        assert!(matches!(reopened.read_shell_run("session","t2").await.unwrap().unwrap().state,
            ShellState::Terminal { outcome: ShellOutcome::Orphaned { .. }, .. }));
        assert_eq!(effect, std::fs::read(temp.path().join("effect.txt")).unwrap());
        reopened.shutdown().await.unwrap();
        }
    }).await.unwrap();
}

#[tokio::test]
async fn stopping_backpressured_pty_keeps_input_prefix_and_waits_for_worker_cleanup() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let temp = tempfile::tempdir().unwrap();
        let log = setup(&temp.path().join("runtime.sqlite")).await;
        let resources = ShellResources::new(log.clone(), CancellationToken::new());
        let mut handle = resources
            .start_pty(
                record(temp.path(), "blocked", "non-reading process"),
                command(
                    temp.path(),
                    "stty raw -echo; printf ready; sleep 60",
                    "Write-Output ready; Start-Sleep -Seconds 60",
                ),
                TerminalSize::new(80, 24).unwrap(),
            )
            .unwrap();
        wait_text(&mut handle, "ready").await;
        let input_handle = handle.clone();
        let input = tokio::spawn(async move {
            input_handle
                .write_raw(
                    "x".repeat(64 * 1024),
                    Some(TerminalSize::new(100, 30).unwrap()),
                )
                .await
        });
        // The input operation must yield under OS backpressure; stop is out of band.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut follower = handle.clone();
        let (first, second) = tokio::join!(handle.stop_and_wait(), follower.stop_and_wait());
        let (first, second) = (first.unwrap(), second.unwrap());
        assert_eq!(usize::from(first.applied) + usize::from(second.applied), 1);
        assert_eq!(first.record, second.record);
        assert!(!handle.stop_and_wait().await.unwrap().applied);
        let result = input.await.unwrap();
        match result {
            Ok(receipt) => {
                assert_eq!(receipt.accepted_bytes, 64 * 1024);
                assert!(receipt.resized);
                assert!(receipt.resize_changed);
                assert_eq!(
                    screen(&receipt.record).size,
                    TerminalSize::new(100, 30).unwrap()
                );
            }
            Err(error) => {
                assert!(
                    error.accepted_bytes.is_some_and(|n| n < 64 * 1024),
                    "{error}"
                );
                assert_eq!(error.resized, Some(true));
                assert_eq!(error.resize_changed, Some(true));
            }
        }
        let final_record = handle.finished().await.unwrap();
        assert!(
            matches!(
                final_record.state,
                ShellState::Terminal {
                    outcome: ShellOutcome::Cancelled { .. },
                    ..
                }
            ),
            "{final_record:?}"
        );
        resources.shutdown().await;
        assert_eq!(resources.active_count(), 0);
        assert_eq!(
            *final_record,
            log.read_shell_run("session", "blocked")
                .await
                .unwrap()
                .unwrap()
        );
        log.shutdown().await.unwrap();
    })
    .await
    .unwrap();
}
