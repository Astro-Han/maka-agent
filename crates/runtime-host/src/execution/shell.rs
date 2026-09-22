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
use maka_process::{PipeEvent, SHELL_NAME, ShellExecutor};
use maka_runtime::{
    shell_run::{ShellOutput, ShellPatch, ShellRun, ShellState, ShellVisibility},
    terminal::{TerminalScreen, TerminalSize},
    tool_output::ToolSuccess,
    tools::ToolError,
};
use maka_tools::{PreparationFuture, PreparedEffect, ToolCallContext, ToolPreparer};
use serde::Deserialize;
use serde_json::Value;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
pub(super) use stdin::{NAME as WRITE_STDIN_NAME, schema as write_stdin_schema};
use tokio_util::sync::CancellationToken;

pub(super) fn schema() -> Value {
    schemars::schema_for!(Input).into()
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    #[schemars(length(min = 1, max = 65536))]
    command: String,
    /// Load the shell's login profile. Set false to skip it.
    #[serde(default = "default_login")]
    login: bool,
    #[schemars(range(min = 1, max = 86_400_000))]
    timeout_ms: Option<u64>,
    #[serde(default)]
    run_in_background: bool,
    /// Allocate a terminal; requires run_in_background=true.
    #[serde(default)]
    pty: bool,
    /// Request additional filesystem or network access before this command.
    /// On Linux/Windows, request an existing parent directory when creating a new path.
    additional_permissions: Option<maka_sandbox::grant::Permissions>,
    /// Explain why the additional access is necessary. Required with additional_permissions.
    justification: Option<String>,
}

fn default_login() -> bool {
    true
}

#[derive(Clone)]
pub(super) struct SessionShell {
    executor: ShellExecutor,
    resources: Arc<ShellResources>,
    log: Arc<EventLog>,
    controllers: crate::controllers::Controllers,
    interactions: Arc<crate::server::interactions::Interactions>,
    ceiling: maka_sandbox::Sandbox,
    revision: u64,
}

impl SessionShell {
    pub(super) fn new(
        executor: ShellExecutor,
        resources: Arc<ShellResources>,
        log: Arc<EventLog>,
        controllers: crate::controllers::Controllers,
        interactions: Arc<crate::server::interactions::Interactions>,
        ceiling: maka_sandbox::Sandbox,
        revision: u64,
    ) -> Self {
        Self {
            executor,
            resources,
            log,
            controllers,
            interactions,
            ceiling,
            revision,
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
        cancellation: CancellationToken,
    ) -> PreparationFuture {
        let mut shell = self.clone();
        Box::pin(async move {
            let revision = if name == SHELL_NAME {
                Some(shell.authorize(&input, &context, &cancellation).await?)
            } else {
                None
            };
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
                        interactions,
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
                    let source = input.command.clone();
                    let terminal = input.pty;
                    let compiler = executor.clone().with_login_shell(input.login);
                    let command = tokio::task::spawn_blocking(move || {
                        if terminal {
                            compiler.command_pty(&source)
                        } else {
                            compiler.command_pipes(&source)
                        }
                    })
                    .await
                    .map_err(failed)??;
                    let admission = tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => return Err(failed("Shell operation cancelled")),
                        admission = interactions.own_admission() => admission,
                    };
                    let current = log
                        .get_session::<crate::session::SessionConfiguration>(
                            &context.invocation.session_id,
                        )
                        .await
                        .map_err(persistence)?;
                    cancelled(&cancellation)?;
                    if !current.is_some_and(|record| {
                        !record.archived && Some(record.configuration.boundary_revision) == revision
                    }) {
                        return Err(failed(
                            "Session permissions changed before command admission",
                        ));
                    }
                    if !input.run_in_background {
                        let mut process = command
                            .observe(Some(input.timeout_ms.unwrap_or(120_000)), cancellation)?;
                        let mut admission = Some(admission);
                        let mut events_open = true;
                        let captured = loop {
                            tokio::select! {
                                biased;
                                event = process.events.recv(), if events_open => match event {
                                    Some(PipeEvent::Started) => { admission.take(); }
                                    Some(PipeEvent::Output { .. }) => {}
                                    None => events_open = false,
                                },
                                result = &mut process.completion => break result?,
                            }
                        };
                        return Ok(captured
                            .into_output(executor.cwd().to_owned(), input.command)
                            .into());
                    }
                    // The native prepared process validates the command before admission.
                    // Only this Host-issued context determines durable source identity.
                    let at = now()?;
                    let size = TerminalSize::new(80, 24).unwrap();
                    let record = ShellRun {
                        id: uuid::Uuid::new_v4().to_string(),
                        session_id: context.invocation.session_id.clone(),
                        source_run_id: Some(context.invocation.run_id.clone()),
                        source_turn_id: context.invocation.turn_id.clone(),
                        source_tool_call_id: context.tool_use_id(),
                        visibility: ShellVisibility::Model,
                        permissions: maka_runtime::shell_run::ShellPermissions {
                            boundary_revision: revision
                                .expect("Shell authorization captured revision"),
                            sandbox: executor.sandbox().clone(),
                        },
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
                    let mut handle = match input.pty {
                        true => resources.start_pty(record, command, size),
                        false => resources.start_pipes(record, command),
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
                    drop(admission);
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

impl SessionShell {
    async fn authorize(
        &mut self,
        input: &Value,
        context: &ToolCallContext,
        cancellation: &CancellationToken,
    ) -> Result<u64, maka_runtime::tool_call::ToolRejection> {
        use maka_runtime::{
            interaction::{PermissionCommand, PermissionRequest},
            tool_call::ToolRejection,
        };
        let invalid = |error: String| ToolRejection::InvalidInput { message: error };
        let failed = |error: String| ToolRejection::PreparationFailed { message: error };
        let mut input: Input =
            serde_json::from_value(input.clone()).map_err(|error| invalid(error.to_string()))?;
        maka_process::validate_command(&input.command)
            .map_err(|error| invalid(error.to_string()))?;
        if input.pty && !input.run_in_background {
            return Err(invalid("PTY mode requires run_in_background=true".into()));
        }
        if input.timeout_ms.is_some_and(|value| {
            value == 0
                || value
                    > if input.run_in_background {
                        86_400_000
                    } else {
                        600_000
                    }
        }) {
            return Err(invalid(
                "Shell timeout must be 1..=600000 ms for foreground or 1..=86400000 ms for background".into(),
            ));
        }
        if input.additional_permissions.is_some() != input.justification.is_some() {
            return Err(invalid(
                "additional_permissions and justification must be supplied together".into(),
            ));
        }
        if cancellation.is_cancelled() {
            return Err(ToolRejection::Cancelled);
        }
        if let Some(permissions) = input.additional_permissions.take() {
            input.additional_permissions = Some(
                tokio::task::spawn_blocking(move || super::permissions::materialize(permissions))
                    .await
                    .map_err(|error| failed(error.to_string()))?
                    .map_err(|error| invalid(error.to_string()))?,
            );
        }
        if let Some(permissions) = &input.additional_permissions
            && !self
                .ceiling
                .permits(permissions)
                .map_err(|error| invalid(error.to_string()))?
        {
            return Err(ToolRejection::PolicyDenied {
                message: "The requested access includes Host-protected resources".into(),
            });
        }
        let current = self
            .log
            .get_session::<crate::session::SessionConfiguration>(&context.invocation.session_id)
            .await
            .map_err(|error| failed(error.to_string()))?
            .filter(|record| !record.archived)
            .ok_or_else(|| failed("Session permissions are unavailable".into()))?;
        let revision = current.configuration.boundary_revision;
        if revision != self.revision {
            return Err(ToolRejection::PolicyDenied {
                message:
                    "Session permissions changed during command preparation; retry the command"
                        .into(),
            });
        }
        let grants = self
            .log
            .permission_grants(&context.invocation, Some(&context.tool_use_id()), revision)
            .await
            .map_err(|error| failed(error.to_string()))?;
        let mut sandbox = self.executor.sandbox().clone();
        for grant in grants {
            sandbox = sandbox
                .with_grant(&grant.permissions, &self.ceiling)
                .map_err(|error| failed(error.to_string()))?;
        }
        if let Some(permissions) = input.additional_permissions
            && !sandbox
                .permits(&permissions)
                .map_err(|error| invalid(error.to_string()))?
        {
            let request = PermissionRequest {
                reason: input.justification.expect("validated justification"),
                command: Some(PermissionCommand {
                    command: input.command,
                    cwd: self.executor.cwd().to_str().expect("validated cwd").into(),
                }),
                permissions,
            };
            let grant = self
                .interactions
                .request_permissions(
                    &context.invocation,
                    Some(&context.tool_use_id()),
                    request,
                    revision,
                    cancellation,
                )
                .await?;
            sandbox = sandbox
                .with_grant(&grant.permissions, &self.ceiling)
                .map_err(|error| failed(error.to_string()))?;
        }
        self.executor = self.executor.with_sandbox(sandbox);
        Ok(revision)
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
