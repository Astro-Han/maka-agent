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

//! Native process primitives executing captured, optionally sandboxed commands.
//! Durable background/PTY resource ownership belongs to the runtime.

#[cfg(windows)]
pub mod bootstrap;
mod command;
pub mod detached;
mod environment;
pub mod pipe;
pub use command::Command;
#[cfg(target_os = "linux")]
pub mod network_namespace;
mod output;
pub mod system;
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
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

// The wire name describes the capability, not a platform-specific executable.
pub const SHELL_NAME: &str = "Shell";
pub const SHELL_DESCRIPTION: &str = "Execute a command with the authorized filesystem and network access. Returns bounded stdout/stderr tails.";
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
    schemars::schema_for!(Input).into()
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    #[schemars(length(min = 1, max = MAX_COMMAND_BYTES))]
    command: String,
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 1, max = MAX_TIMEOUT_MS))]
    timeout_ms: u64,
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

#[derive(Clone)]
pub struct ShellExecutor {
    cwd: PathBuf,
    shell: shell::ShellPlan,
    sandbox: maka_sandbox::Sandbox,
    network_route: maka_network::Policy,
    #[cfg(target_os = "linux")]
    network_helper: Option<PathBuf>,
    #[cfg(windows)]
    backend: Option<std::sync::Arc<dyn bootstrap::Backend>>,
}

impl ShellExecutor {
    #[cfg(target_os = "linux")]
    pub fn with_network_helper(mut self, helper: PathBuf) -> Self {
        self.network_helper = Some(helper);
        self
    }
    pub fn with_network_route(mut self, route: maka_network::Policy) -> Self {
        self.network_route = route;
        self
    }
    #[cfg(windows)]
    pub fn with_backend(mut self, backend: std::sync::Arc<dyn bootstrap::Backend>) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn sandbox(&self) -> &maka_sandbox::Sandbox {
        &self.sandbox
    }

    /// Replace only the authorized policy; retain the captured cwd and shell.
    pub fn with_sandbox(&self, sandbox: maka_sandbox::Sandbox) -> Self {
        Self {
            sandbox,
            ..self.clone()
        }
    }

    /// Capture an already authorized isolation policy. This does not authorize
    /// execution; the Host still owns admission and durable dispatch.
    pub fn new(cwd: impl AsRef<Path>, sandbox: maka_sandbox::Sandbox) -> Result<Self, ToolError> {
        let cwd = std::fs::canonicalize(cwd).map_err(|e| failed(e.to_string()))?;
        #[cfg(windows)]
        let cwd = dunce::simplified(&cwd).to_owned();
        if !cwd.is_dir() || cwd.to_str().is_none() {
            return Err(failed("Shell cwd must be a UTF-8 directory"));
        }
        Ok(Self {
            cwd,
            shell: shell::ShellPlan::detect(),
            sandbox,
            network_route: maka_network::Policy::default(),
            #[cfg(target_os = "linux")]
            network_helper: None,
            #[cfg(windows)]
            backend: None,
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
    pub fn interactive_pty(&self) -> Result<(String, pty::PtyCommand), ToolError> {
        let (source, command) = self.shell.interactive(&self.cwd);
        Ok((source, self.capture(command)?))
    }

    /// The selected dialect and quoting match pipes, but stdin is a terminal.
    /// Capturing a plan performs no native effect.
    pub fn command_pty(&self, command: &str) -> Result<pty::PtyCommand, ToolError> {
        validate_command(command)?;
        self.prepare(command, true)
    }

    /// Capture a non-interactive launch and its authorized sandbox policy.
    pub fn command_pipes(&self, command: &str) -> Result<Command, ToolError> {
        validate_command(command)?;
        self.prepare(command, false)
    }

    fn prepare(&self, source: &str, terminal: bool) -> Result<Command, ToolError> {
        self.capture(self.shell.command(&self.cwd, source, terminal))
    }

    fn capture(&self, command: Command) -> Result<Command, ToolError> {
        #[cfg(target_os = "linux")]
        let command = match &self.network_helper {
            Some(helper) => command.with_network_helper(helper.clone()),
            None => command,
        };
        #[cfg(windows)]
        let command = match &self.backend {
            Some(backend) => command.with_backend(backend.clone()),
            None => command,
        };
        command
            .network_route(self.network_route.clone())
            .sandbox(&self.sandbox)
            .map_err(|e| failed(e.to_string()))
    }

    /// Prepare a background pipe execution. No process is started until
    /// completion is polled; the Host must first commit durable dispatch.
    pub fn observe(
        &self,
        command: String,
        timeout_ms: Option<u64>,
        cancellation: CancellationToken,
    ) -> Result<ObservedProcess, ToolError> {
        self.command_pipes(&command)?
            .observe(timeout_ms, cancellation)
    }
}

impl Command {
    /// No native resources exist until completion is polled. The accepted worker
    /// owns sandbox preparation, process startup and cleanup as one lifecycle.
    pub fn observe(
        self,
        timeout_ms: Option<u64>,
        cancellation: CancellationToken,
    ) -> Result<ObservedProcess, ToolError> {
        if timeout_ms.is_some_and(|ms| !(1..=86_400_000).contains(&ms)) {
            return Err(failed("invalid background shell command or timeout"));
        }
        let (observer, events) = tokio::sync::mpsc::channel(8);
        let completion = Box::pin(async move {
            let cancellation = cancellation.child_token();
            let _cancel_on_drop = cancellation.clone().drop_guard();
            tokio::spawn(process::run(self, timeout_ms, cancellation, Some(observer)))
                .await
                .map_err(|e| ToolError::CleanupUnconfirmed(format!("Shell worker lost: {e}")))?
        });
        Ok(ObservedProcess { events, completion })
    }
}

impl ToolExecutor for ShellExecutor {
    fn names(&self) -> Vec<String> {
        vec![SHELL_NAME.into()]
    }

    fn invoke(&self, name: String, input: Value, cancellation: CancellationToken) -> ToolFuture {
        let executor = self.clone();
        Box::pin(async move {
            if name != SHELL_NAME {
                return Err(failed("unsupported tool"));
            }
            let input: Input = serde_json::from_value(input).map_err(|e| failed(e.to_string()))?;
            validate_command(&input.command)?;
            if !(1..=MAX_TIMEOUT_MS).contains(&input.timeout_ms) {
                return Err(failed(
                    "Shell requires a nonempty bounded command and timeout_ms in 1..=600000",
                ));
            }
            let cancellation = cancellation.child_token();
            let plan = executor.prepare(&input.command, false)?;
            // Dropping the awaiter requests cancellation, but cannot abort the
            // worker that owns process termination, reaping and pipe drain.
            let _cancel_on_drop = cancellation.clone().drop_guard();
            let captured = tokio::spawn(process::run(
                plan,
                Some(input.timeout_ms),
                cancellation,
                None,
            ))
            .await
            .map_err(|e| ToolError::CleanupUnconfirmed(format!("Shell worker lost: {e}")))??;
            Ok(captured.into_output(executor.cwd, input.command))
        })
    }
}

fn failed(message: impl Into<String>) -> ToolError {
    ToolError::Failed(message.into())
}

pub fn validate_command(command: &str) -> Result<(), ToolError> {
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
