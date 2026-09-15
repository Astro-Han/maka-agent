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

use maka_js_runtime::CodeExecutor;
use maka_runtime::event::{EventSink, Invocation};
use maka_runtime::model::ModelToolCall;
use maka_runtime::tool_call::{ToolCallIdentity, ToolRejection};
use maka_runtime::tool_output::ToolSuccess;
use maka_runtime::tools::{ToolError, ToolExecutor, ToolJournal};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::availability::{Availability, SEARCH};
use crate::{
    PreparedEffect, ToolCallContext, ToolCatalog, ToolDefinition, ToolMode, ToolSemantics, cell,
};

/// Run-scoped authority. Cell capacity is shared across runs, not recreated here.
pub struct RunTools {
    journal: ToolJournal,
    availability: Availability,
    mode: ToolMode,
    cells: CodeExecutor,
}

impl RunTools {
    pub fn new(
        sink: Arc<dyn EventSink>,
        invocation: Invocation,
        catalog: ToolCatalog,
        mode: ToolMode,
        cells: CodeExecutor,
    ) -> Self {
        Self {
            journal: ToolJournal::new(sink, invocation),
            availability: Availability::new(catalog),
            mode,
            cells,
        }
    }

    pub fn clear_loaded(&self) {
        self.availability.clear();
    }

    pub fn checkpoint(&self) -> maka_runtime::handoff::HandoffTools {
        self.availability.checkpoint()
    }

    pub fn restore(
        &self,
        checkpoint: &maka_runtime::handoff::HandoffTools,
    ) -> Result<(), ToolError> {
        self.availability.restore(checkpoint)
    }

    /// Capture once per logical model step, before sending any physical attempt.
    pub fn capture(&self) -> RequestTools<'_> {
        let availability = if self.mode == ToolMode::CodeMode {
            self.availability.nested()
        } else {
            self.availability.clone()
        };
        RequestTools {
            run: self,
            catalog: availability.snapshot(),
            availability,
        }
    }
}

/// The advertised schemas and their handlers share one captured capability view.
/// Search settlement affects future captures, never this request or its retries.
pub struct RequestTools<'a> {
    run: &'a RunTools,
    catalog: ToolCatalog,
    availability: Availability,
}

impl<'a> RequestTools<'a> {
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions: Vec<_> = self.catalog.definitions().cloned().collect();
        if let Some(search) = self.availability.definition() {
            definitions.push(search);
        }
        if self.run.mode == ToolMode::CodeMode {
            let mut exec = cell::definition();
            exec.description.push_str("\nThis is the only callable tool. After searching, return its result and use the refreshed catalog in the next exec call. Available nested functions:\n");
            exec.description.push_str(
                &serde_json::to_string(&definitions).expect("function definitions are JSON"),
            );
            vec![exec]
        } else {
            definitions
        }
    }

    pub fn into_step(self, step_id: &'a str) -> StepTools<'a> {
        StepTools {
            request: self,
            step_id,
            admission: Admission::Fresh,
        }
    }
}

/// Call-order admission is separate from execution scheduling. Serial execution
/// alone does not make exclusive-step siblings legal.
pub struct StepTools<'a> {
    request: RequestTools<'a>,
    step_id: &'a str,
    admission: Admission,
}

#[derive(Default)]
enum Admission {
    #[default]
    Fresh,
    Parallel,
    Exclusive,
}

impl Admission {
    fn admit(&mut self, semantics: ToolSemantics) -> Result<(), ToolRejection> {
        *self = match (&self, semantics) {
            (Self::Fresh, ToolSemantics::ExclusiveStep) => Self::Exclusive,
            (Self::Fresh | Self::Parallel, ToolSemantics::Parallel) => Self::Parallel,
            _ => return Err(ToolRejection::ExclusiveConflict),
        };
        Ok(())
    }
}

impl StepTools<'_> {
    pub async fn invoke(
        &mut self,
        call: &ModelToolCall,
        cancellation: CancellationToken,
    ) -> Result<Value, ToolError> {
        let run = self.request.run;
        let operation_id = format!("{}:{}", self.step_id, call.id);
        let identity = ToolCallIdentity::provider(self.step_id.into(), call.id.clone());
        let preparation: Result<PreparedEffect, ToolRejection> = async {
            if cancellation.is_cancelled() {
                return Err(ToolRejection::Cancelled);
            }
            if run.mode == ToolMode::CodeMode && call.name != "exec" {
                return Err(ToolRejection::Unavailable);
            }
            if call.name == "exec" && run.mode == ToolMode::CodeMode {
                self.admission.admit(ToolSemantics::ExclusiveStep)?;
                let executor = cell::CellTool::new(
                    run.cells.clone(),
                    cell::source(&call.input)?,
                    self.request.catalog.clone(),
                    self.request.availability.clone(),
                    run.journal.clone(),
                    operation_id.clone(),
                    call.id.clone(),
                );
                let effect: PreparedEffect = Box::new(move |cancellation| {
                    Box::pin(async move {
                        executor
                            .invoke("exec".into(), Value::Null, cancellation)
                            .await
                            .map(ToolSuccess::from)
                    })
                });
                Ok(effect)
            } else if call.name == SEARCH && self.request.availability.enabled() {
                self.admission.admit(ToolSemantics::Parallel)?;
                self.request.availability.prepare_search(&call.input)
            } else {
                self.admission
                    .admit(self.request.catalog.semantics(&call.name)?)?;
                self.request
                    .catalog
                    .prepare(
                        call.name.clone(),
                        call.input.clone(),
                        ToolCallContext {
                            invocation: run.journal.invocation().clone(),
                            operation_id: operation_id.clone(),
                        },
                        cancellation.clone(),
                    )
                    .await
            }
        }
        .await;
        let effect = match preparation {
            Ok(effect) => effect,
            Err(reason) => {
                return run
                    .journal
                    .reject(
                        operation_id,
                        identity,
                        call.name.clone(),
                        call.input.clone(),
                        reason,
                    )
                    .await;
            }
        };
        let result = run
            .journal
            .invoke_call_with(
                operation_id,
                identity,
                call.name.clone(),
                call.input.clone(),
                cancellation,
                effect,
            )
            .await?;
        self.request.availability.settled(&call.name, &result)?;
        Ok(result)
    }
}
