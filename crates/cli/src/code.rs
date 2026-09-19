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

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use maka_event_log::EventLog;
use maka_js_runtime::{CellAbort, CellDiagnosticKind, CellLimits, CellResult, CodeExecutor};
use maka_runtime::event::{EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent};
use maka_runtime::execution::{
    BehaviorId, CollaborationMode, InvocationConfiguration, PermissionMode, ToolMode,
};
use maka_runtime::tools::{JournaledTools, ToolError, ToolExecutor, ToolFuture};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    path: String,
    content: String,
}

struct LocalTools;

impl ToolExecutor for LocalTools {
    fn names(&self) -> Vec<String> {
        vec!["echo".into(), "read_file".into(), "write_file".into()]
    }

    fn invoke(&self, name: String, input: Value, cancel: CancellationToken) -> ToolFuture {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                if cancel.is_cancelled() {
                    return Err(ToolError::Failed("cancelled before effect".into()));
                }
                let failed = |error: serde_json::Error| ToolError::Failed(error.to_string());
                match name.as_str() {
                    "echo" => Ok(input),
                    "read_file" => {
                        let input: ReadInput = serde_json::from_value(input).map_err(failed)?;
                        let file = std::fs::File::open(input.path)
                            .map_err(|error| ToolError::Failed(error.to_string()))?;
                        let mut bytes = Vec::new();
                        file.take(1024 * 1024 + 1)
                            .read_to_end(&mut bytes)
                            .map_err(|error| ToolError::Failed(error.to_string()))?;
                        if bytes.len() > 1024 * 1024 {
                            return Err(ToolError::Failed("file read exceeds 1 MiB".into()));
                        }
                        let text = String::from_utf8(bytes)
                            .map_err(|error| ToolError::Failed(error.to_string()))?;
                        Ok(json!({"text": text}))
                    }
                    "write_file" => {
                        let input: WriteInput = serde_json::from_value(input).map_err(failed)?;
                        // OS sandboxing is deliberately deferred. A failed write
                        // may already have changed the file: do not call it safe
                        // to retry, and do not synthesize a known failed outcome.
                        std::fs::write(input.path, &input.content)
                            .map_err(|error| ToolError::OutcomeUnknown(error.to_string()))?;
                        Ok(json!({"bytes": input.content.len()}))
                    }
                    _ => Err(ToolError::Failed(format!("unknown tool: {name}"))),
                }
            })
            .await
            .map_err(|error| ToolError::OutcomeUnknown(error.to_string()))?
        })
    }
}

pub(super) async fn run(path: &Path) -> Result<(), maka_runtime_host::server::HostError> {
    let log = Arc::new(EventLog::open(path).await?);
    let limits = CellLimits::default();
    let mut source = String::new();
    std::io::stdin()
        .take(limits.max_source_bytes as u64 + 1)
        .read_to_string(&mut source)?;
    if source.len() > limits.max_source_bytes {
        return Err("code source exceeds 64 KiB".into());
    }
    let id = || Uuid::new_v4().to_string();
    let invocation = Invocation {
        session_id: id(),
        turn_id: id(),
        run_id: id(),
        invocation_id: id(),
    };
    log.append(&EventWrite::plain(RuntimeEvent::new(
        invocation.clone(),
        Fact::InvocationOpened {
            configuration: Some(Box::new(InvocationConfiguration {
                system_prompt: None,
                tool_composition: None,
                workspace_identity: None,
                cwd: std::env::current_dir()?
                    .into_os_string()
                    .into_string()
                    .map_err(|_| "code working directory is not UTF-8")?,
                permission_mode: PermissionMode::Bypass,
                collaboration_mode: CollaborationMode::Agent,
                orchestration_mode: BehaviorId::default(),
                tool_mode: ToolMode::CodeMode,
                model: None,
                thinking_level: None,
            })),
            input: maka_runtime::input::InvocationInput::Code {
                source: source.clone(),
            },
        },
    ))?)
    .await?;
    let engine = CodeExecutor::new(1, limits)?;
    let tools = Arc::new(JournaledTools::new(
        log.clone(),
        invocation.clone(),
        Arc::new(LocalTools),
    ));
    let cancellation = CancellationToken::new();
    let signal_cancel = cancellation.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_cancel.cancel();
        }
    });
    let result = engine.execute(source, tools, cancellation).await;
    signal.abort();
    let outcome = match &result {
        Ok(CellResult::Success { .. }) => InvocationOutcome::Completed,
        Ok(CellResult::Failure { error, .. }) => InvocationOutcome::Failed {
            class: match error.kind {
                CellDiagnosticKind::ParseError => "cell_parse",
                CellDiagnosticKind::ExecutionError => "javascript",
                CellDiagnosticKind::UnknownTool => "unknown_tool",
                CellDiagnosticKind::LimitExceeded => "cell_limit",
                CellDiagnosticKind::ToolFailure => "tool_execution",
            }
            .into(),
            message: Some(error.message.chars().take(2048).collect()),
        },
        Err(CellAbort::Cancelled) => InvocationOutcome::Cancelled {
            source: "runtime_cancellation".into(),
        },
        Err(error) => InvocationOutcome::Failed {
            class: match error {
                CellAbort::Tool(_) => "tool_execution",
                CellAbort::Cancelled => unreachable!(),
                CellAbort::Internal(_) => "cell_internal",
            }
            .into(),
            message: Some(error.to_string().chars().take(2048).collect()),
        },
    };
    log.append(&EventWrite::plain(RuntimeEvent::new(
        invocation.clone(),
        Fact::InvocationEnded { outcome },
    ))?)
    .await?;
    log.shutdown().await?;
    match result {
        Ok(output) => {
            println!("{}", json!({"invocation": invocation, "output": output}));
            if let CellResult::Failure { error, .. } = output {
                return Err(error.message.into());
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
