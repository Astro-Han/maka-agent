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

use std::sync::Arc;

use maka_js_runtime::{CellAbort, CodeExecutor};
use maka_runtime::tool_call::{ToolCallIdentity, ToolOrigin, ToolRejection};
use maka_runtime::tools::{ToolError, ToolExecutor, ToolFuture, ToolJournal};
use serde::Deserialize;
use serde_json::{Value, json};
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
        description: "Run bounded JavaScript in a fresh runtime. Call available tools with tools.<name>(args), use await or Promise.all, and return a JSON value.".into(),
        input_schema: json!({
            "type":"object", "properties":{"code":{"type":"string"}},
            "required":["code"], "additionalProperties":false,
        }),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeInput {
    code: String,
}

pub(super) fn source(input: &Value) -> Result<String, ToolRejection> {
    serde_json::from_value::<CodeInput>(input.clone())
        .map(|input| input.code)
        .map_err(|_| ToolRejection::InvalidInput {
            message: "exec requires exactly one string field: code".into(),
        })
}

pub(super) struct CellTool {
    cells: CodeExecutor,
    code: String,
    nested: Arc<NestedTools>,
}

impl CellTool {
    pub(super) fn new(
        cells: CodeExecutor,
        code: String,
        catalog: ToolCatalog,
        availability: Availability,
        journal: ToolJournal,
        parent_operation_id: String,
        parent_tool_call_id: String,
    ) -> Self {
        Self {
            cells,
            code,
            nested: Arc::new(NestedTools {
                catalog,
                availability,
                journal,
                origin: ToolOrigin::CodeMode {
                    parent_operation_id,
                    parent_tool_call_id,
                },
            }),
        }
    }
}

impl ToolExecutor for CellTool {
    fn names(&self) -> Vec<String> {
        vec!["exec".into()]
    }

    fn invoke(&self, _: String, _: Value, cancellation: CancellationToken) -> ToolFuture {
        let cells = self.cells.clone();
        let code = self.code.clone();
        let nested = self.nested.clone();
        Box::pin(async move {
            // The parent journal has already committed T1. execute returns only
            // after nested effects drain; its ordinary diagnostics are JSON values.
            match cells.execute(code, nested, cancellation).await {
                Ok(result) => serde_json::to_value(result)
                    .map_err(|error| ToolError::OutcomeUnknown(error.to_string())),
                Err(CellAbort::Tool(error)) => Err(error),
                Err(CellAbort::Cancelled) => {
                    Err(ToolError::Failed("code execution cancelled".into()))
                }
                Err(CellAbort::Internal(message)) => Err(ToolError::CleanupUnconfirmed(message)),
            }
        })
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
