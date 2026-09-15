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

mod control;
use super::{
    Result, ShellError, Update,
    control::{Control, ControlError},
    handle::StopState,
    output::Output,
    terminal::Terminal,
};
use futures_util::FutureExt;
use maka_event_log::EventLog;
use maka_process::pty::{self, PtyChild, PtyCommand, PtyIo};
use maka_runtime::{
    shell_run::{ShellOutcome, ShellOutput, ShellPatch, ShellRun, ShellState},
    terminal::TerminalSize,
};
use std::{
    num::NonZeroI64,
    process::ExitStatus,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

pub(super) struct Worker {
    pub runtime: maka_js_runtime::trusted::TrustedRuntime,
    pub log: Arc<EventLog>,
    pub record: ShellRun,
    pub commands: mpsc::Receiver<Control>,
    pub updates: watch::Sender<Update>,
    pub cancellation: CancellationToken,
    pub stop: Arc<Mutex<StopState>>,
    pub host_drain: CancellationToken,
    pub persistence_failed: bool,
    pub output: Output,
}
impl Worker {
    pub async fn run(mut self, command: PtyCommand, size: TerminalSize) -> Result<()> {
        let mut terminal = Terminal::new(self.runtime.clone(), size, self.output.clone()).await?;
        let result = self.run_terminal(command, size, &mut terminal).await;
        terminal.close().await;
        result
    }

    async fn run_terminal(
        &mut self,
        command: PtyCommand,
        size: TerminalSize,
        terminal: &mut Terminal,
    ) -> Result<()> {
        self.record.output = ShellOutput::Pty {
            screen: terminal.snapshot.clone(),
        };
        if self.cancellation.is_cancelled() {
            return Err(ShellError::CancelledBeforeAdmission);
        }
        // T1: neither a native handle nor a process exists before this settles.
        self.record = match self.log.create_shell_run(self.record.clone()).await {
            Ok(record) => record,
            Err(error) => {
                if matches!(
                    error,
                    maka_event_log::StoreError::CommitUnknown(_)
                        | maka_event_log::StoreError::OperationUnknown
                ) {
                    self.host_drain.cancel();
                }
                return Err(error.into());
            }
        };
        let spawned = if self.cancellation.is_cancelled() {
            Err(std::io::Error::other("PTY launch cancelled before spawn"))
        } else {
            pty::spawn(command, size).await
        };
        let (mut child, io) = match spawned {
            Ok(handles) => handles,
            Err(error) => {
                return self
                    .persist(terminal, Some(finished(failed(&error))?))
                    .await;
            }
        };
        let result = match self.persist(terminal, Some(ShellState::Running)).await {
            Ok(()) => self.active(terminal, &mut child, &io).await,
            Err(error) => Err(error),
        };
        self.commands.close();
        terminal.fail_writes("PTY is closing");
        // Pending commands have performed no effect; acknowledge that explicitly.
        while let Ok(control) = self.commands.try_recv() {
            let _ = control
                .reply
                .send(Err(ControlError::new("PTY is closing", 0)));
        }
        let mut outcome = result.unwrap_or_else(|error| failed(&error));
        if !matches!(
            outcome,
            ShellOutcome::Completed | ShellOutcome::Exited { .. }
        ) {
            match child.terminate() {
                Ok(applied) => {
                    self.stop.lock().unwrap().signal_applied =
                        Some(matches!(outcome, ShellOutcome::Cancelled { .. }) && applied);
                }
                Err(error) => {
                    self.stop.lock().unwrap().signal_applied = None;
                    outcome = failed(&error);
                    self.host_drain.cancel();
                }
            }
        }
        // Root exit, console close and final output are distinct fences. Never
        // await ClosePseudoConsole without a concurrent consumer of its output.
        let exited = CancellationToken::new();
        let closing = async {
            let result = child.wait().await;
            exited.cancel();
            result?;
            child.close().await
        };
        let draining = super::terminal::drain(terminal, &io, &exited);
        let (closed, drained) = tokio::join!(closing, draining);
        if let Err(error) = closed {
            self.host_drain.cancel();
            outcome = failed(&error);
        }
        if let Err(error) = drained {
            outcome = failed(&error);
        }
        drop(io);
        drop(child);
        terminal.fail_writes("PTY closed");
        if self.persistence_failed {
            // A commit-unknown is not repaired by writing a guessed next state.
            return Err(ShellError::Rejected(
                "PTY persistence failed; durable outcome requires recovery",
            ));
        }
        // T2 and final screen become visible together, after native cleanup.
        self.persist(terminal, Some(finished(outcome)?)).await
    }

    async fn active(
        &mut self,
        terminal: &mut Terminal,
        child: &mut PtyChild,
        io: &PtyIo,
    ) -> Result<ShellOutcome> {
        let timeout_ms = self.record.timeout_ms;
        let timeout = async move {
            match timeout_ms {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(timeout);
        let mut buffer = [0; 16 * 1024];
        let mut eof = false;
        loop {
            tokio::select! {
                _ = self.cancellation.cancelled() => return Ok(ShellOutcome::Cancelled { message: None }),
                _ = &mut timeout => return Ok(ShellOutcome::TimedOut { message: None }),
                status = child.wait() => return Ok(exit_outcome(status?)),
                read = io.read(&mut buffer), if !eof => {
                    let mut count = read?;
                    eof = count == 0;
                    // Coalesce only ready bytes, within the existing 16 KiB cut.
                    // Both native readers are cancellation-safe; never wait here
                    // for a fuller batch or delay a pending input/stop for one.
                    let mut failure = None;
                    while !eof && count < buffer.len() {
                        match io.read(&mut buffer[count..]).now_or_never() {
                            Some(Ok(0)) => eof = true,
                            Some(Ok(read)) => count += read,
                            Some(Err(error)) => { failure = Some(error); break; }
                            None => break,
                        }
                    }
                    // Parser cuts are awaited to completion outside select.
                    terminal.output(&buffer[..count], eof).await?;
                    self.persist(terminal, None).await?;
                    if let Some(error) = failure { return Err(error.into()); }
                }
                written = async { io.write(terminal.writes.front().unwrap().remaining()).await }, if !terminal.writes.is_empty() => {
                    let count = written?;
                    if count == 0 { return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into()); }
                    if terminal.writes.front_mut().unwrap().advance(count) {
                        terminal.writes.pop_front().unwrap().finish(self.current_cut());
                    }
                }
                command = self.commands.recv(), if terminal.writes.is_empty() => {
                    match command {
                        Some(control) => self.control(terminal, child, control).await?,
                        None => return Ok(ShellOutcome::Cancelled { message: None }),
                    }
                }
            }
        }
    }

    async fn persist(&mut self, terminal: &Terminal, state: Option<ShellState>) -> Result<()> {
        let output = ShellOutput::Pty {
            screen: terminal.snapshot.clone(),
        };
        if state.is_none() && output == self.record.output {
            return Ok(());
        }
        let patch = ShellPatch {
            state,
            output: Some(output),
            updated_at: Some(now()?),
            ..Default::default()
        };
        match self
            .log
            .patch_shell_run(&self.record.session_id, &self.record.id, patch)
            .await
        {
            Ok(record) => {
                self.record = record;
                self.updates
                    .send_replace(Some(Ok(Arc::new(self.record.clone()))));
                Ok(())
            }
            Err(error) => {
                self.persistence_failed = true;
                self.host_drain.cancel();
                Err(error.into())
            }
        }
    }
}

pub(super) fn now() -> Result<u64> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ShellError::Rejected("clock predates Unix epoch"))?
            .as_millis(),
    )
    .map_err(|_| ShellError::Rejected("clock exceeds shell timestamp range"))
}
pub(super) fn finished(outcome: ShellOutcome) -> Result<ShellState> {
    Ok(ShellState::Terminal {
        completed_at: now()?,
        outcome,
        observed_at: None,
    })
}
pub(super) fn failed(error: &impl std::fmt::Display) -> ShellOutcome {
    ShellOutcome::Failed {
        message: error.to_string().chars().take(512).collect(),
    }
}
pub(super) fn exit_outcome(status: ExitStatus) -> ShellOutcome {
    if status.success() {
        return ShellOutcome::Completed;
    }
    #[cfg(unix)]
    let code = {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .map(i64::from)
            .or_else(|| status.signal().map(|s| 128 + i64::from(s)))
    };
    #[cfg(windows)]
    let code = status.code().map(|code| i64::from(code as u32));
    match code.and_then(NonZeroI64::new) {
        Some(code) => ShellOutcome::Exited {
            code,
            message: None,
        },
        None => failed(&"PTY exited without an exit code"),
    }
}
