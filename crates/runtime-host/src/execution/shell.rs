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
mod stdin;
use crate::shell::{ShellError, ShellResources};
pub(super) use control::{STOP_NAME, stop_schema};
use maka_event_log::EventLog;
use maka_presentation::shell::{RESOURCE_REF_PREFIX, local_update};
use maka_process::{SHELL_NAME, ShellExecutor};
use maka_runtime::{
    shell_run::{ShellOutput, ShellPatch, ShellRun, ShellState, ShellVisibility},
    terminal::{TerminalScreen, TerminalSize},
    tool_output::ToolSuccess,
    tools::{ToolError, ToolExecutor},
};
use maka_tools::{PreparationFuture, PreparedEffect, ToolCallContext, ToolPreparer};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
pub(super) use stdin::{NAME as WRITE_STDIN_NAME, schema as write_stdin_schema};
use tokio_util::sync::CancellationToken;

pub(super) fn schema() -> Value {
    let mut schema = maka_process::shell_schema();
    schema["properties"]["timeout_ms"]["maximum"] = json!(86_400_000);
    schema["properties"]["run_in_background"] = json!({"type":"boolean"});
    schema["properties"]["pty"] = json!({"type":"boolean","description":"Allocate a terminal; requires run_in_background=true."});
    schema
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    command: String,
    timeout_ms: Option<u64>,
    #[serde(default)]
    run_in_background: bool,
    #[serde(default)]
    pty: bool,
}

#[derive(Clone)]
pub(super) struct SessionShell {
    executor: ShellExecutor,
    resources: Arc<ShellResources>,
    log: Arc<EventLog>,
    controllers: crate::controllers::Controllers,
}

impl SessionShell {
    pub(super) fn new(
        executor: ShellExecutor,
        resources: Arc<ShellResources>,
        log: Arc<EventLog>,
        controllers: crate::controllers::Controllers,
    ) -> Self {
        Self {
            executor,
            resources,
            log,
            controllers,
        }
    }
}

impl ToolPreparer for SessionShell {
    fn names(&self) -> Vec<String> {
        vec![SHELL_NAME.into(), STOP_NAME.into(), WRITE_STDIN_NAME.into()]
    }

    fn prepare(
        &self,
        name: String,
        input: Value,
        context: ToolCallContext,
        _cancellation: CancellationToken,
    ) -> PreparationFuture {
        let shell = self.clone();
        Box::pin(async move {
            let effect: PreparedEffect = PreparedEffect::new(move |cancellation| {
                Box::pin(async move {
                    cancelled(&cancellation)?;
                    if name == WRITE_STDIN_NAME {
                        return shell
                            .write_stdin(&context.invocation.session_id, input, cancellation)
                            .await;
                    }
                    let Self {
                        executor,
                        resources,
                        log,
                        ..
                    } = shell;
                    if name == STOP_NAME {
                        return control::stop(
                            &resources,
                            &log,
                            &context.invocation.session_id,
                            input,
                            &cancellation,
                        )
                        .await;
                    }
                    if name != SHELL_NAME {
                        return Err(failed("unsupported shell tool"));
                    }
                    let input: Input = serde_json::from_value(input).map_err(failed)?;
                    if input.pty && !input.run_in_background {
                        return Err(failed("PTY mode requires run_in_background=true"));
                    }
                    if !input.run_in_background {
                        let mut args = json!({"command":input.command});
                        if let Some(timeout) = input.timeout_ms {
                            args["timeout_ms"] = json!(timeout);
                        }
                        return executor
                            .invoke(name, args, cancellation)
                            .await
                            .map(ToolSuccess::from);
                    }
                    // The native prepared process validates the command before admission.
                    // Only this Host-issued context determines durable source identity.
                    let at = now()?;
                    let size = TerminalSize::new(80, 24).unwrap();
                    let pty = input
                        .pty
                        .then(|| executor.command_pty(&input.command))
                        .transpose()?;
                    let record = ShellRun {
                        id: uuid::Uuid::new_v4().to_string(),
                        session_id: context.invocation.session_id.clone(),
                        source_run_id: Some(context.invocation.run_id.clone()),
                        source_turn_id: context.invocation.turn_id.clone(),
                        source_tool_call_id: context.tool_use_id(),
                        visibility: ShellVisibility::Model,
                        cwd: executor.cwd().to_str().expect("validated shell cwd").into(),
                        command: input.command,
                        started_at: at,
                        updated_at: at,
                        timeout_ms: input.timeout_ms,
                        revision: 1,
                        state: ShellState::Starting,
                        output: if input.pty {
                            ShellOutput::Pty {
                                screen: TerminalScreen::new(size),
                            }
                        } else {
                            ShellOutput::Pipes {
                                stdout: String::new(),
                                stderr: String::new(),
                                latest_stream: None,
                                stdout_truncated: false,
                                stderr_truncated: false,
                            }
                        },
                    };
                    let mut handle = match pty {
                        Some(command) => resources.start_pty(record, command, size),
                        None => resources.start_pipes(record, executor),
                    }
                    .map_err(failed)?;
                    // Once accepted, never abandon native startup/cleanup. Cancellation
                    // before handoff owns the task; after handoff only the Host does.
                    let record = tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => {
                            handle.stop();
                            handle.finished().await.map_err(worker_error)?
                        }
                        ready = handle.ready() => ready.map_err(worker_error)?,
                    };
                    let record = if cancellation.is_cancelled() {
                        handle.stop();
                        handle.finished().await.map_err(worker_error)?
                    } else {
                        record
                    };
                    let compact = record.state.active();
                    project(observed(&log, (*record).clone()).await?, compact)
                })
            });
            Ok(effect)
        })
    }
}

pub(super) fn parse_ref(reference: &str) -> Option<&str> {
    reference
        .strip_prefix(RESOURCE_REF_PREFIX)
        .filter(|id| maka_runtime::interaction::entity_id(id).is_ok())
}

pub(super) async fn read(
    log: &EventLog,
    session: &str,
    id: &str,
    cancellation: &CancellationToken,
) -> Result<maka_presentation::shell::ShellSnapshot, ToolError> {
    let record = read_model(log, session, id).await?;
    cancelled(cancellation)?;
    // Each native parser/output cut is already committed by its sole worker.
    // SQL is the observation boundary, not a second live snapshot authority.
    local_update(observed(log, record).await?)
        .map(|update| update.result)
        .map_err(persistence)
}

async fn read_model(log: &EventLog, session: &str, id: &str) -> Result<ShellRun, ToolError> {
    log.read_shell_run(session, id)
        .await
        .map_err(persistence)?
        .filter(|record| record.visibility == ShellVisibility::Model)
        .ok_or_else(|| failed("Runtime background task not found in this session"))
}

async fn observed(log: &EventLog, record: ShellRun) -> Result<ShellRun, ToolError> {
    if !matches!(
        record.state,
        ShellState::Terminal {
            observed_at: None,
            ..
        }
    ) {
        return Ok(record);
    }
    log.patch_shell_run(
        &record.session_id,
        &record.id,
        ShellPatch {
            observed_at: Some(now()?),
            ..Default::default()
        },
    )
    .await
    .map_err(persistence)
}

fn project(record: ShellRun, compact: bool) -> Result<ToolSuccess, ToolError> {
    let mut snapshot = local_update(record).map_err(persistence)?.result;
    if compact {
        snapshot.output = None;
    }
    serde_json::to_value(snapshot)
        .map(ToolSuccess::from)
        .map_err(persistence)
}

fn now() -> Result<u64, ToolError> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(failed)?
            .as_millis(),
    )
    .map_err(failed)
}
fn cancelled(token: &CancellationToken) -> Result<(), ToolError> {
    if token.is_cancelled() {
        Err(failed("Shell operation cancelled"))
    } else {
        Ok(())
    }
}
fn failed(error: impl std::fmt::Display) -> ToolError {
    ToolError::Failed(error.to_string())
}
fn persistence(error: impl std::fmt::Display) -> ToolError {
    ToolError::Persistence(error.to_string())
}
fn worker_error(error: Arc<ShellError>) -> ToolError {
    match error.as_ref() {
        ShellError::CancelledBeforeAdmission => failed(error),
        ShellError::Store(error) => persistence(error),
        ShellError::Process(error) => error.clone(),
        _ => ToolError::CleanupUnconfirmed(error.to_string()),
    }
}
