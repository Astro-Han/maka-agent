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

mod declarations;
mod session;
pub(crate) use session::Cells;
pub(super) fn declarations(definitions: &[ToolDefinition]) -> String {
    declarations::render(definitions)
}

use maka_js_runtime::CodeExecutor;
use maka_runtime::tool_call::{ToolCallIdentity, ToolOrigin, ToolRejection};
use maka_runtime::tools::{ToolExecutor, ToolFuture, ToolJournal};
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    ToolCallContext, ToolCatalog, ToolDefinition,
    availability::{Availability, SEARCH},
};
use uuid::Uuid;

pub(super) fn definition() -> ToolDefinition {
    ToolDefinition {
        provider: None,
        name: "exec".into(),
        description: r#"Run an async JavaScript function body in a fresh bounded V8 isolate; no Node, filesystem, network or console.
Call tools.<name>(args) or tools["name"](args). Await every intended operation. Promise.all queues work: 8 tools run concurrently, with 32 calls total per cell.
Return JSON, or emit selected output using text(value). image(value) accepts a Host image result, MCP image block, or base64 data URL. generatedImage({image_url, output_hint?}) emits generated image content.
audio(value) accepts an MCP audio block or base64 data URL; bytes are retained in evidence, but current model adapters receive audio metadata, not native audio input.
notify(value) emits text and requests an immediate observation. await yield_control() returns accumulated output while the cell continues. exit() ends the JS body successfully; admitted Host effects still settle.
Results have state running/completed/terminated. For running, use wait with cell_id to receive only new output or request termination. Up to 4 uncollected cells per Run.
store(key, value) and load(key) retain bounded JSON between cells in this Run, not JS globals. Values are published after cell settlement. Compaction, Run end and Host restart clear them.
ALL_TOOLS is this cell's frozen name/description catalog. Search cannot add tools to a running cell; use the refreshed catalog in the next exec.
setTimeout(callback, millis) and clearTimeout(id) are available; timers alone do not keep a completed cell alive. Synchronous execution is bounded to 30 seconds; asynchronous tool waits do not consume it. Cancellation and Run completion stop cells and settle accepted effects."#.into(),
        input_schema: schemars::schema_for!(CodeInput).into(),
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CodeInput {
    /// Async JavaScript function body, at most 64 KiB UTF-8. Return JSON or emit content.
    code: String,
    /// Observation wait in milliseconds (0..60000), not an execution timeout.
    #[serde(default = "default_yield")]
    #[schemars(range(min = 0, max = 60000))]
    yield_time_ms: u64,
    /// Approximate text-output budget (64..32768 tokens). Full output is retained as evidence.
    #[serde(default = "default_tokens")]
    #[schemars(range(min = 64, max = 32768))]
    max_output_tokens: usize,
}

fn default_yield() -> u64 {
    10_000
}
fn default_tokens() -> usize {
    4_000
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WaitInput {
    /// The opaque cell_id returned by exec or wait in this Run.
    cell_id: String,
    /// Observation wait in milliseconds (0..60000), not an execution timeout.
    #[serde(default = "default_yield")]
    #[schemars(range(min = 0, max = 60000))]
    yield_time_ms: u64,
    /// Approximate text-output budget (64..32768 tokens).
    #[serde(default = "default_tokens")]
    #[schemars(range(min = 64, max = 32768))]
    max_output_tokens: usize,
    /// Request cancellation. Running means accepted effects are still being settled.
    #[serde(default)]
    terminate: bool,
}

pub(super) fn wait_definition() -> ToolDefinition {
    ToolDefinition {
        provider: None, name: "wait".into(),
        description: "Observe a running Code Mode cell using the cell_id from exec/wait. Returns only new output and its running/completed/terminated state. terminate requests cancellation; while cleanup is pending the state stays running. Completed cells are collected once. IDs are scoped to this Run and do not survive Host restart. yield_time_ms is 0..60000 (default 10000); max_output_tokens is an approximate text budget, 64..32768 (default 4000).".into(),
        input_schema: schemars::schema_for!(WaitInput).into(),
    }
}

fn invalid(message: impl Into<String>) -> ToolRejection {
    ToolRejection::InvalidInput {
        message: message.into(),
    }
}

fn observation_limits(yield_time_ms: u64, tokens: usize) -> Result<(), ToolRejection> {
    if yield_time_ms > 60_000 || !(64..=32_768).contains(&tokens) {
        return Err(invalid(
            "yield_time_ms must be 0..60000; max_output_tokens must be 64..32768",
        ));
    }
    Ok(())
}

pub(super) fn source(input: &Value) -> Result<CodeInput, ToolRejection> {
    serde_json::from_value::<CodeInput>(input.clone())
        .map_err(|error| invalid(error.to_string()))
        .and_then(|input| {
            observation_limits(input.yield_time_ms, input.max_output_tokens)?;
            if input.code.len() > 64 * 1024 {
                return Err(invalid("code exceeds 64 KiB"));
            }
            Ok(input)
        })
}

pub(super) fn wait_input(input: &Value) -> Result<WaitInput, ToolRejection> {
    let input: WaitInput =
        serde_json::from_value(input.clone()).map_err(|error| invalid(error.to_string()))?;
    observation_limits(input.yield_time_ms, input.max_output_tokens)?;
    Ok(input)
}

pub(super) struct CellTool {
    cells: CodeExecutor,
    input: CodeInput,
    catalog: ToolCatalog,
    availability: Availability,
    journal: ToolJournal,
    parent_operation_id: String,
    parent_tool_call_id: String,
}

impl CellTool {
    pub(super) fn new(
        cells: CodeExecutor,
        input: CodeInput,
        catalog: ToolCatalog,
        availability: Availability,
        journal: ToolJournal,
        parent_operation_id: String,
        parent_tool_call_id: String,
    ) -> Self {
        Self {
            cells,
            input,
            catalog,
            availability,
            journal,
            parent_operation_id,
            parent_tool_call_id,
        }
    }
}

/// Nested preflight precedes T1. The cell supplies only arguments, never its
/// parent identity or a broader catalog. Direct-only entries were removed once.
struct NestedTools {
    catalog: ToolCatalog,
    availability: Availability,
    journal: ToolJournal,
    origin: ToolOrigin,
}

impl ToolExecutor for NestedTools {
    fn names(&self) -> Vec<String> {
        let mut names = self.catalog.names();
        if self.availability.enabled() {
            names.push(SEARCH.into());
        }
        names
    }

    fn invoke(&self, name: String, input: Value, cancellation: CancellationToken) -> ToolFuture {
        let catalog = self.catalog.clone();
        let availability = self.availability.clone();
        let journal = self.journal.clone();
        let call = ToolCallIdentity {
            tool_call_id: Uuid::new_v4().to_string(),
            origin: self.origin.clone(),
        };
        let operation_id = Uuid::new_v4().to_string();
        Box::pin(async move {
            let context = ToolCallContext {
                invocation: journal.invocation().clone(),
                operation_id: operation_id.clone(),
            };
            let prepared = if name == SEARCH && availability.enabled() {
                availability.prepare_search(&input)
            } else {
                catalog
                    .prepare(name.clone(), input.clone(), context, cancellation.clone())
                    .await
            };
            let effect = match prepared {
                Ok(effect) => effect,
                Err(reason) => {
                    return journal
                        .reject(operation_id, call, name, input, reason)
                        .await;
                }
            };
            let result = journal
                .invoke_prepared_call(
                    operation_id,
                    call,
                    name.clone(),
                    input,
                    cancellation,
                    effect,
                )
                .await?;
            availability.settled(&name, &result)?;
            Ok(result)
        })
    }
}
