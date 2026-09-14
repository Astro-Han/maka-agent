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

use crate::{
    PipeEvent, failed,
    output::{Captured, Outcome, Termination},
    shell::ShellPlan,
    tail,
};
use maka_runtime::shell_run::PipeStream;
use maka_runtime::tools::ToolError;
use std::{io, path::PathBuf, time::Duration};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

const GRACE: Duration = Duration::from_secs(2);
const REAP: Duration = Duration::from_secs(2);

pub(crate) async fn run(
    cwd: PathBuf,
    _shell: ShellPlan,
    command: String,
    timeout_ms: Option<u64>,
    cancellation: CancellationToken,
    observer: Option<tokio::sync::mpsc::Sender<PipeEvent>>,
) -> Result<Captured, ToolError> {
    if cancellation.is_cancelled() {
        return Err(failed("Bash cancelled before spawn"));
    }
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(&command)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| failed(format!("Bash spawn failed: {e}")))?;
    let pid = child.id().expect("spawned child has a pid");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    if let Some(observer) = &observer {
        // This is always the first event in a newly bounded channel.
        let _ = observer.try_send(PipeEvent::Started);
    }
    let exited = CancellationToken::new();
    let lifecycle = async {
        let outcome = settle(&mut child, pid, timeout_ms, cancellation).await;
        exited.cancel();
        outcome
    };
    let (outcome, stdout, stderr) = tokio::join!(
        lifecycle,
        tail::capture(
            stdout,
            exited.clone(),
            observer.clone().map(|o| (o, PipeStream::Stdout))
        ),
        tail::capture(
            stderr,
            exited.clone(),
            observer.map(|o| (o, PipeStream::Stderr))
        )
    );
    Ok(Captured::new(outcome?, stdout, stderr))
}

async fn settle(
    child: &mut Child,
    pid: u32,
    timeout_ms: Option<u64>,
    cancellation: CancellationToken,
) -> Result<Outcome, ToolError> {
    let reason = tokio::select! {
        biased;
        result = child.wait() => return result.map(Outcome::Exited).map_err(unknown),
        _ = cancellation.cancelled() => Termination::Cancelled,
        _ = crate::timeout(timeout_ms) => Termination::Timeout,
    };
    let signal_applied = signal_group(pid, libc::SIGTERM);
    // Do not reap the root during grace: its unreaped PID pins the group
    // identity until our final signal, even if it exits before descendants.
    // Captured pipes continue draining concurrently in run().
    tokio::time::sleep(GRACE).await;
    signal_group(pid, libc::SIGKILL);
    // A failed signal attempt is not exit evidence; still wait/reap.
    let _ = child.start_kill();
    tokio::time::timeout(REAP, child.wait())
        .await
        .map_err(|_| ToolError::OutcomeUnknown("Bash root exit not confirmed after KILL".into()))?
        .map_err(unknown)?;
    Ok(Outcome::Interrupted {
        reason,
        signal_applied,
    })
}

fn signal_group(pid: u32, signal: libc::c_int) -> bool {
    // The child creates its own process group before exec. No shell arguments
    // influence this target. This does not reach escaped sessions/groups.
    unsafe { libc::kill(-(pid as libc::pid_t), signal) == 0 }
}
fn unknown(error: io::Error) -> ToolError {
    ToolError::OutcomeUnknown(format!("Bash root exit not confirmed: {error}"))
}
