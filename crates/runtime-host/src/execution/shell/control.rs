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

use super::{cancelled, failed, observed, parse_ref, persistence, read_model, worker_error};
use crate::shell::ShellResources;
use maka_event_log::EventLog;
use maka_presentation::shell::{ShellSnapshot, local_update};
use maka_runtime::{
    shell_run::{ShellOutcome, ShellRun, ShellState},
    terminal::TerminalSize,
    tool_output::ToolSuccess,
    tools::ToolError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub const STOP_NAME: &str = "StopBackgroundTask";

pub fn stop_schema() -> Value {
    schemars::schema_for!(StopInput).into()
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct StopInput {
    /// Background shell ref returned by Shell.
    #[serde(rename = "ref")]
    reference: String,
}

#[derive(Serialize)]
struct ControlResult {
    #[serde(flatten)]
    snapshot: ShellSnapshot,
    operation: Operation,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Operation {
    Stop {
        applied: bool,
    },
    PtyControl {
        failed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<InputReceipt>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resize: Option<ResizeReceipt>,
    },
}

#[derive(Serialize)]
struct InputReceipt {
    bytes: usize,
    queued: bool,
}

#[derive(Serialize)]
struct ResizeReceipt {
    #[serde(flatten)]
    size: TerminalSize,
    applied: bool,
    changed: bool,
}

pub(super) async fn stop(
    resources: &ShellResources,
    log: &EventLog,
    session: &str,
    input: Value,
    cancellation: &CancellationToken,
) -> Result<ToolSuccess, ToolError> {
    let input: StopInput = serde_json::from_value(input).map_err(failed)?;
    let id = parse_ref(&input.reference).ok_or_else(|| failed("Invalid background task ref"))?;
    // Durable Session and visibility, not possession of a ref, grant authority.
    let mut record = read_model(log, session, id).await?;
    cancelled(cancellation)?;
    let applied = if let Some(mut handle) = resources.get(session, id) {
        // No input gate: stopping must remain possible under native backpressure.
        // After acceptance, the journal-owned effect waits through cleanup even
        // if its requesting Turn is cancelled.
        let receipt = handle.stop_and_wait().await.map_err(worker_error)?;
        record = (*receipt.record).clone();
        receipt.applied
    } else {
        if record.state.active() {
            // The owner may have completed between the SQL read and live lookup.
            // Startup recovery, never a tool waiter, owns orphan classification.
            record = read_model(log, session, id).await?;
            if record.state.active() {
                return Err(ToolError::CleanupUnconfirmed(
                    "active shell has no native owner".into(),
                ));
            }
        }
        cancelled(cancellation)?;
        false
    };
    project(log, record, Operation::Stop { applied }).await
}

async fn project(
    log: &EventLog,
    record: ShellRun,
    operation: Operation,
) -> Result<ToolSuccess, ToolError> {
    let snapshot = local_update(observed(log, record).await?)
        .map_err(persistence)?
        .result;
    serde_json::to_value(ControlResult {
        snapshot,
        operation,
    })
    .map(ToolSuccess::from)
    .map_err(persistence)
}

impl super::SessionShell {
    pub(super) async fn write_stdin(
        &self,
        session: &str,
        value: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolSuccess, ToolError> {
        use crate::shell::ControlErrorKind;
        let input = super::stdin::Input::parse(value)?;
        let id = parse_ref(&input.reference).expect("validated ref");
        let mut record = read_model(&self.log, session, id).await?;
        if !record.output.is_pty() {
            return Err(failed("WriteStdin requires a PTY background task ref"));
        }
        cancelled(&cancellation)?;
        let mut queued = false;
        let mut resized = false;
        let mut changed = false;
        let mut operation_failed = false;
        if let Some(mut handle) = self.resources.get(session, id) {
            let gate = handle.control_gate.clone();
            let _guard = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(failed("PTY control cancelled before admission")),
                guard = gate.lock() => guard,
            };
            if self.controllers.lock().leases.iter().any(|lease| {
                lease.identity.session_id == session
                    && lease.identity.resource_ref == input.reference
            }) {
                return Err(failed("This PTY is controlled by a connected Client"));
            }
            cancelled(&cancellation)?;
            record = (*handle.ready().await.map_err(worker_error)?).clone();
            if record.state.active() {
                let admission = tokio::select! {
                    _ = cancellation.cancelled() => return Err(failed("PTY control cancelled before admission")),
                    gate = self.interactions.own_admission() => gate,
                };
                let current = self
                    .log
                    .get_session::<crate::session::SessionConfiguration>(session)
                    .await
                    .map_err(persistence)?
                    .filter(|session| !session.archived)
                    .ok_or_else(|| failed("Session permissions are unavailable"))?;
                if current.configuration.boundary_revision != record.permissions.boundary_revision {
                    return Err(failed(
                        "PTY launch permissions no longer match this Session; start a new terminal",
                    ));
                }
                let result = handle.enqueue_control(input.data, input.size, cancellation);
                drop(admission);
                let result = match result {
                    Ok(receipt) => receipt.await,
                    Err(error) => Err(error),
                };
                match result {
                    Ok(receipt) => {
                        record = (*receipt.record).clone();
                        queued = input.bytes.is_some();
                        resized = receipt.resized;
                        changed = receipt.resize_changed;
                    }
                    Err(error) => match error.kind {
                        ControlErrorKind::Rejected => return Err(failed(error)),
                        ControlErrorKind::Unknown => {
                            return Err(ToolError::CleanupUnconfirmed(error.to_string()));
                        }
                        ControlErrorKind::Closed => {
                            handle.stop();
                            record = (*handle.finished().await.map_err(worker_error)?).clone();
                            resized = error.resized.expect("known control closure");
                            changed = error.resize_changed.expect("known control closure");
                            operation_failed = error.accepted_bytes != Some(0)
                                || resized
                                || !matches!(
                                    record.state,
                                    ShellState::Terminal {
                                        outcome: ShellOutcome::Completed
                                            | ShellOutcome::Exited { .. },
                                        ..
                                    }
                                );
                        }
                    },
                }
            }
        } else if record.state.active() {
            record = read_model(&self.log, session, id).await?;
            if record.state.active() {
                return Err(ToolError::CleanupUnconfirmed(
                    "active PTY has no native owner".into(),
                ));
            }
            cancelled(&cancellation)?;
        }
        project(
            &self.log,
            record,
            Operation::PtyControl {
                failed: operation_failed,
                input: input.bytes.map(|bytes| InputReceipt { bytes, queued }),
                resize: input.size.map(|size| ResizeReceipt {
                    size,
                    applied: resized,
                    changed,
                }),
            },
        )
        .await
    }
}
