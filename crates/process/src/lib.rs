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

//! Trusted native process primitives and foreground execution, not a sandbox.
//! Durable background/PTY resource ownership belongs to the runtime.

mod output;
pub use output::{Captured as PipeResult, Outcome as ProcessOutcome, Termination};
#[cfg(unix)]
mod process;
pub mod pty;
mod shell;
mod tail;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as process;

use maka_runtime::tools::{ToolError, ToolExecutor, ToolFuture};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

// Keep the established wire tool name; the executor is not tied to Bash.
pub const SHELL_NAME: &str = "Bash";
pub const SHELL_DESCRIPTION: &str =
    "Execute a command with unrestricted host access. Returns bounded stdout/stderr tails.";
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_COMMAND_BYTES: usize = 65_536;

/// Bounded observations, not durable resource facts. Started precedes output.
#[derive(Debug)]
pub enum PipeEvent {
    Started,
    Output {
        stream: maka_runtime::shell_run::PipeStream,
        bytes: Vec<u8>,
    },
}

pub type PipeFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<PipeResult, ToolError>> + Send>>;

pub struct ObservedProcess {
    pub events: tokio::sync::mpsc::Receiver<PipeEvent>,
    /// Poll concurrently with events. Completion includes native cleanup and
    /// bounded pipe drain; dropping this future requests cancellation.
    pub completion: PipeFuture,
}

pub fn shell_schema() -> Value {
    json!({"type":"object","properties":{
        "command":{"type":"string","minLength":1,"maxLength":MAX_COMMAND_BYTES},
        "timeout_ms":{"type":"integer","minimum":1,"maximum":MAX_TIMEOUT_MS}
    },"required":["command"],"additionalProperties":false})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    command: String,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

#[derive(Clone)]
pub struct ShellExecutor {
    cwd: PathBuf,
    shell: shell::ShellPlan,
}

impl ShellExecutor {
    /// Grants unrestricted OS command execution. Only trusted Host policy may
    /// construct this executor; cwd is a working directory, not a boundary.
    pub fn trusted_unrestricted(cwd: impl AsRef<Path>) -> Result<Self, ToolError> {
        let cwd = std::fs::canonicalize(cwd).map_err(|e| failed(e.to_string()))?;
        if !cwd.is_dir() || cwd.to_str().is_none() {
            return Err(failed("Bash cwd must be a UTF-8 directory"));
        }
        Ok(Self {
            cwd,
            shell: shell::ShellPlan::detect(),
        })
    }

    pub fn description(&self) -> String {
        format!("{SHELL_DESCRIPTION} {}", self.shell.guidance())
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Capture the selected interactive login shell and its display command.
    /// The caller still owns durable admission before native PTY spawn.
    pub fn interactive_pty(&self) -> (String, pty::PtyCommand) {
        self.shell.interactive(&self.cwd)
    }

    /// The selected dialect and quoting match pipes, but stdin is a terminal.
    /// Capturing a plan performs no native effect.
    pub fn command_pty(&self, command: &str) -> Result<pty::PtyCommand, ToolError> {
        validate_command(command)?;
        Ok(self.shell.pty_command(&self.cwd, command))
    }

    /// Prepare a trusted background pipe execution. No process is started until
    /// completion is polled; the Host must first commit durable dispatch.
    pub fn observe(
        &self,
        command: String,
        timeout_ms: Option<u64>,
        cancellation: CancellationToken,
    ) -> Result<ObservedProcess, ToolError> {
        validate_command(&command)?;
        if timeout_ms.is_some_and(|ms| !(1..=86_400_000).contains(&ms)) {
            return Err(failed("invalid background shell command or timeout"));
        }
        let (observer, events) = tokio::sync::mpsc::channel(8);
        let cwd = self.cwd.clone();
        let shell = self.shell.clone();
        let completion = Box::pin(async move {
            let cancellation = cancellation.child_token();
            let _cancel_on_drop = cancellation.clone().drop_guard();
            tokio::spawn(process::run(
                cwd,
                shell,
                command,
                timeout_ms,
                cancellation,
                Some(observer),
            ))
            .await
            .map_err(|e| ToolError::OutcomeUnknown(format!("Shell worker lost: {e}")))?
        });
        Ok(ObservedProcess { events, completion })
    }
}

impl ToolExecutor for ShellExecutor {
    fn names(&self) -> Vec<String> {
        vec![SHELL_NAME.into()]
    }

    fn invoke(&self, name: String, input: Value, cancellation: CancellationToken) -> ToolFuture {
        let cwd = self.cwd.clone();
        let shell = self.shell.clone();
        Box::pin(async move {
            if name != SHELL_NAME {
                return Err(failed("unsupported tool"));
            }
            let input: Input = serde_json::from_value(input).map_err(|e| failed(e.to_string()))?;
            validate_command(&input.command)?;
            if !(1..=MAX_TIMEOUT_MS).contains(&input.timeout_ms) {
                return Err(failed(
                    "Bash requires a nonempty bounded command and timeout_ms in 1..=600000",
                ));
            }
            let cancellation = cancellation.child_token();
            // Dropping the awaiter requests cancellation, but cannot abort the
            // worker that owns process termination, reaping and pipe drain.
            let _cancel_on_drop = cancellation.clone().drop_guard();
            let captured = tokio::spawn(process::run(
                cwd.clone(),
                shell,
                input.command.clone(),
                Some(input.timeout_ms),
                cancellation,
                None,
            ))
            .await
            .map_err(|e| ToolError::OutcomeUnknown(format!("Bash worker lost: {e}")))??;
            Ok(output::render(cwd, input, captured))
        })
    }
}

fn failed(message: impl Into<String>) -> ToolError {
    ToolError::Failed(message.into())
}

fn validate_command(command: &str) -> Result<(), ToolError> {
    if command.trim().is_empty() || command.len() > MAX_COMMAND_BYTES || command.contains('\0') {
        return Err(failed(
            "Shell requires a nonempty command of at most 64 KiB without NUL",
        ));
    }
    Ok(())
}

async fn timeout(ms: Option<u64>) {
    match ms {
        Some(ms) => tokio::time::sleep(std::time::Duration::from_millis(ms)).await,
        None => std::future::pending().await,
    }
}
