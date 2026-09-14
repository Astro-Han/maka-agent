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

use super::live_shell::command;
use super::live_shell::{record, setup};
use maka_event_log::EventLog;
use maka_runtime::{
    shell_run::{ShellOutcome, ShellOutput, ShellState},
    terminal::TerminalSize,
};
use maka_runtime_host::shell::{ShellHandle, ShellResources};
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

pub(super) fn launch(
    resources: &ShellResources,
    cwd: &Path,
    id: &str,
    pty: bool,
    unix: &str,
    windows: &str,
) -> ShellHandle {
    if pty {
        return resources
            .start_pty(
                record(cwd, id, "create effect"),
                command(cwd, unix, windows),
                TerminalSize::new(80, 24).unwrap(),
            )
            .unwrap();
    }
    #[cfg(unix)]
    let source = unix;
    #[cfg(windows)]
    let source = windows;
    let mut record = record(cwd, id, source);
    record.timeout_ms = None;
    record.output = ShellOutput::Pipes {
        stdout: String::new(),
        stderr: String::new(),
        latest_stream: None,
        stdout_truncated: false,
        stderr_truncated: false,
    };
    resources
        .start_pipes(
            record,
            maka_process::ShellExecutor::trusted_unrestricted(cwd).unwrap(),
        )
        .unwrap()
}

#[tokio::test]
async fn background_pipes_publish_durable_live_output_and_outlive_the_launch_waiter() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime.sqlite");
        let log = setup(&path).await;
        let drain = CancellationToken::new();
        let resources = ShellResources::new(log.clone(), drain.clone());
        let mut handle = launch(&resources, temp.path(), "pipe", false,
            "printf ready; printf early-error >&2; while [ ! -f release ]; do sleep 0.02; done; i=0; while [ $i -lt 24000 ]; do printf 界; i=$((i+1)); done; printf 尾; printf final-error >&2; exit 23",
            "[Console]::Write('ready'); [Console]::Error.Write('early-error'); while (-not (Test-Path release)) { Start-Sleep -Milliseconds 20 }; [Console]::Write(('界' * 24000) + '尾'); [Console]::Error.Write('final-error'); exit 23");
        loop {
            let record = handle.ready().await.unwrap();
            assert!(record.state.active());
            if matches!(&record.output, ShellOutput::Pipes { stdout, stderr, .. } if stdout == "ready" && stderr == "early-error") {
                assert_eq!(*record, log.read_shell_run("session", "pipe").await.unwrap().unwrap());
                break;
            }
            handle.changed().await.unwrap();
        }
        assert!(handle.attach().is_none());
        assert_eq!(handle.write_raw("no input".into(), None).await.unwrap_err().accepted_bytes, Some(0));
        drop(handle);
        assert_eq!(resources.active_count(), 1);
        std::fs::write(temp.path().join("release"), b"continue").unwrap();
        let mut handle = resources.get("session", "pipe").unwrap();
        let record = handle.finished().await.unwrap();
        assert!(matches!(&record.state, ShellState::Terminal { outcome: ShellOutcome::Exited { code, .. }, .. } if code.get() == 23));
        let ShellOutput::Pipes { stdout, stderr, stdout_truncated, stderr_truncated, .. } = &record.output else { panic!("expected pipes") };
        assert!(stdout.len() <= 65_536 && stdout.ends_with('尾') && !stdout.contains('�'));
        assert!(*stdout_truncated);
        assert_eq!(stderr, "early-errorfinal-error");
        assert!(!stderr_truncated);
        assert_eq!(*record, log.read_shell_run("session", "pipe").await.unwrap().unwrap());
        let mut stopped = launch(&resources, temp.path(), "stop", false, "printf running; sleep 60", "[Console]::Write('running'); Start-Sleep -Seconds 60");
        stopped.ready().await.unwrap();
        let mut follower = stopped.clone();
        let (first, second) = tokio::join!(stopped.stop_and_wait(), follower.stop_and_wait());
        let (first, second) = (first.unwrap(), second.unwrap());
        assert_eq!(usize::from(first.applied) + usize::from(second.applied), 1);
        assert_eq!(first.record, second.record);
        assert!(!stopped.stop_and_wait().await.unwrap().applied);
        assert!(matches!(first.record.state,
            ShellState::Terminal { outcome: ShellOutcome::Cancelled { .. }, .. }));
        let mut abandoned = launch(&resources, temp.path(), "abandoned-stop", false,
            "sleep 60", "Start-Sleep -Seconds 60");
        abandoned.ready().await.unwrap();
        {
            let mut accepted = Box::pin(abandoned.stop_and_wait());
            assert!(futures_util::poll!(&mut accepted).is_pending());
            // Drop after synchronous acceptance, before native cleanup/T2.
        }
        let receipt = abandoned.stop_and_wait().await.unwrap();
        assert!(!receipt.applied, "a new waiter cannot claim an accepted stop");
        assert!(matches!(receipt.record.state,
            ShellState::Terminal { outcome: ShellOutcome::Cancelled { .. }, .. }));
        assert_eq!(*receipt.record,
            log.read_shell_run("session", "abandoned-stop").await.unwrap().unwrap());
        resources.shutdown().await;
        assert_eq!(resources.active_count(), 0);
        assert!(!drain.is_cancelled());
        log.shutdown().await.unwrap();
        drop(resources);
        drop(log);
        let reopened = EventLog::open(&path).await.unwrap();
        assert_eq!(reopened.recover_shell_runs(100).await.unwrap(), 0);
        assert_eq!(*record, reopened.read_shell_run("session", "pipe").await.unwrap().unwrap());
        reopened.shutdown().await.unwrap();
    }).await.unwrap();
}
