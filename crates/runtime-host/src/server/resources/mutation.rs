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

use super::{Host, HostError, failure};
use crate::{session::SessionConfiguration, shell::ShellError};
use maka_event_log::sessions::SessionRecord;
use maka_presentation::shell::{RESOURCE_REF_PREFIX, local_update};
use maka_protocol::{
    OperationErrorCode as Code, Outcome,
    resource::{ResourceMutationResult, ResourceStartInput, ResourceStopInput},
};
use maka_runtime::{
    shell_run::{ShellOutcome, ShellOutput, ShellPatch, ShellRun, ShellState, ShellVisibility},
    terminal::{TerminalScreen, TerminalSize},
};

pub(super) async fn start(host: &Host, input: ResourceStartInput) -> Result<Outcome, HostError> {
    if maka_runtime::interaction::entity_id(&input.launch_id).is_err()
        || input
            .command
            .as_ref()
            .is_some_and(|command| command.contains('\0'))
    {
        return Ok(failure(
            Code::InvalidRequest,
            "Invalid shell launch identity or command",
        ));
    }
    // Capture cwd/shell and settle startup inside the relocation/archive gate.
    let _admission = host.executions.lock_admission().await;
    if host.draining.is_cancelled() {
        return Ok(failure(Code::HostDraining, "Host is draining"));
    }
    let session = match active_session(host, &input.session_id).await {
        Ok(session) => session,
        Err(outcome) => return Ok(outcome),
    };
    let cwd = session.configuration.workspace.host_cwd.clone();
    let executor = match tokio::task::spawn_blocking(move || {
        maka_process::ShellExecutor::trusted_unrestricted(cwd)
    })
    .await?
    {
        Ok(executor) => executor,
        Err(error) => return Ok(failure(Code::InvalidRequest, &error.to_string())),
    };
    let started_at = match super::super::configuration::now() {
        Ok(now) => now,
        Err(error) => return Ok(fault(host, error)),
    };
    let size = TerminalSize::new(80, 24).unwrap();
    let (command, pty) = match input.command {
        Some(command) => (command, None),
        None => {
            let (command, plan) = executor.interactive_pty();
            (command, Some(plan))
        }
    };
    let record = ShellRun {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: input.session_id,
        source_run_id: None,
        source_turn_id: input.launch_id.clone(),
        source_tool_call_id: input.launch_id,
        visibility: if pty.is_some() {
            ShellVisibility::Model
        } else {
            ShellVisibility::User
        },
        cwd: session.configuration.workspace.host_cwd,
        command,
        started_at,
        updated_at: started_at,
        timeout_ms: None,
        revision: 1,
        state: ShellState::Starting,
        output: if pty.is_some() {
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
    let launch = match pty {
        Some(command) => host.shells.start_pty(record, command, size),
        None => host.shells.start_pipes(record, executor),
    };
    let mut handle = match launch {
        Ok(handle) => handle,
        Err(ShellError::Rejected(message)) => return Ok(failure(Code::OperationConflict, message)),
        Err(error) => return Ok(fault(host, error)),
    };
    match handle.ready().await {
        Ok(record) => match projected((*record).clone()) {
            Ok(result) => Ok(result),
            Err(error) => {
                handle.stop();
                let _ = handle.finished().await;
                Ok(fault(host, error))
            }
        },
        Err(error) => Ok(fault(host, error)),
    }
}

pub(super) async fn stop(host: &Host, input: ResourceStopInput) -> Result<Outcome, HostError> {
    let Some(id) = input
        .resource_ref
        .strip_prefix(RESOURCE_REF_PREFIX)
        .filter(|id| maka_runtime::interaction::entity_id(id).is_ok())
    else {
        return Ok(failure(
            Code::InvalidRequest,
            "Runtime Resource ref is unsupported",
        ));
    };
    // Only the stop decision is gated. Waiting for a backpressured process to
    // settle must not hold admission for unrelated Sessions or Host shutdown.
    let handle = {
        let _admission = host.executions.lock_admission().await;
        if let Err(outcome) = active_session(host, &input.session_id).await {
            return Ok(outcome);
        }
        let handle = host.shells.get(&input.session_id, id);
        if let Some(handle) = &handle {
            handle.stop();
        }
        handle
    };
    let record = match handle {
        Some(mut handle) => match handle.finished().await {
            Ok(record) => (*record).clone(),
            Err(error) => return Ok(fault(host, error)),
        },
        None => match host.log.read_shell_run(&input.session_id, id).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                return Ok(failure(
                    Code::NotFound,
                    "Runtime Resource was not found in this Session",
                ));
            }
            Err(error) => return Ok(fault(host, error)),
        },
    };
    let now = match super::super::configuration::now() {
        Ok(now) => now,
        Err(error) => return Ok(fault(host, error)),
    };
    let state = record.state.active().then_some(ShellState::Terminal {
        completed_at: now,
        outcome: ShellOutcome::Orphaned {
            message: "Runtime has no live shell process handle".into(),
        },
        observed_at: None,
    });
    let record = match host
        .log
        .patch_shell_run(
            &input.session_id,
            id,
            ShellPatch {
                state,
                observed_at: Some(now),
                ..Default::default()
            },
        )
        .await
    {
        Ok(record) => record,
        Err(error) => return Ok(fault(host, error)),
    };
    Ok(projected(record).unwrap_or_else(|error| fault(host, error)))
}

pub(super) async fn active_session(
    host: &Host,
    id: &str,
) -> Result<SessionRecord<SessionConfiguration>, Outcome> {
    match host.log.get_session(id).await {
        Ok(Some(session)) if session.archived => {
            Err(failure(Code::SessionArchived, "Session is archived"))
        }
        Ok(Some(session)) => Ok(session),
        Ok(None) => Err(failure(Code::NotFound, "Session was not found")),
        Err(error) => Err(fault(host, error)),
    }
}

fn projected(record: ShellRun) -> Result<Outcome, HostError> {
    let value = serde_json::to_value(ResourceMutationResult {
        resource: local_update(record)?.result,
    })?;
    maka_protocol::resource::decode_mutation_result(&value)?;
    Ok(Outcome::success(value))
}

pub(super) fn fault(host: &Host, error: impl std::fmt::Display) -> Outcome {
    host.draining.cancel();
    eprintln!("Runtime Resource operation failed: {error}");
    failure(Code::InternalFailure, "Runtime Resource operation failed")
}
