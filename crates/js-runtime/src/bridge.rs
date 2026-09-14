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

use crate::result::diagnostic_fits;
use crate::{CellDiagnostic, CellDiagnosticKind, CellLimits, ToolCall};
use deno_core::{OpState, op2};
use maka_runtime::tools::{ToolError, ToolExecutor};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::HashSet,
    future::Future,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tokio::{runtime::Handle, sync::oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Default)]
pub(crate) struct Admission {
    pub(crate) calls: Vec<ToolCall>,
    in_flight: usize,
    pub(crate) fatal: Option<ToolError>,
}

pub(crate) struct ToolScope {
    pub(crate) executor: Arc<dyn ToolExecutor>,
    pub(crate) names: HashSet<String>,
    pub(crate) cancellation: CancellationToken,
    pub(crate) tasks: TaskTracker,
    pub(crate) host: Handle,
    pub(crate) limits: CellLimits,
    pub(crate) admission: Mutex<Admission>,
}

impl ToolScope {
    fn start(
        self: &Arc<Self>,
        name: String,
        input: Value,
    ) -> Result<oneshot::Receiver<Result<Value, CellDiagnostic>>, CellDiagnostic> {
        if self.cancellation.is_cancelled() {
            return Err(CellDiagnostic::new(
                CellDiagnosticKind::ExecutionError,
                "cell cancelled",
            ));
        }
        if !self.names.contains(&name) {
            return Err(CellDiagnostic::new(
                CellDiagnosticKind::UnknownTool,
                format!("unknown tool: {name}"),
            ));
        }
        if serde_json::to_vec(&input)
            .map_err(|e| CellDiagnostic::new(CellDiagnosticKind::ExecutionError, e.to_string()))?
            .len()
            > self.limits.max_value_bytes
        {
            return Err(CellDiagnostic::limit("tool input byte limit exceeded"));
        }
        {
            let mut admission = self.admission.lock().unwrap();
            if admission.fatal.is_some() || self.cancellation.is_cancelled() {
                return Err(CellDiagnostic::new(
                    CellDiagnosticKind::ExecutionError,
                    "cell cancelled",
                ));
            }
            if admission.calls.len() >= self.limits.max_tool_calls
                || admission.in_flight >= self.limits.max_in_flight_tools
            {
                return Err(CellDiagnostic::limit("tool admission limit exceeded"));
            }
            let index = admission.calls.len() + 1;
            let mut reserved = admission.calls.clone();
            reserved.push(ToolCall {
                index,
                name: name.clone(),
            });
            if !diagnostic_fits(&reserved, self.limits.max_value_bytes) {
                return Err(CellDiagnostic::limit("tool summary byte limit exceeded"));
            }
            admission.calls = reserved;
            admission.in_flight += 1;
        }
        let (sender, receiver) = oneshot::channel();
        let scope = self.clone();
        self.tasks.spawn_on(
            async move {
                use deno_core::futures::FutureExt;
                let result = std::panic::AssertUnwindSafe(async {
                    scope
                        .executor
                        .invoke(name, input, scope.cancellation.clone())
                        .await
                })
                .catch_unwind()
                .await;
                let result = result.unwrap_or_else(|_| {
                    Err(ToolError::OutcomeUnknown("tool executor panicked".into()))
                });
                {
                    let mut admission = scope.admission.lock().unwrap();
                    admission.in_flight -= 1;
                    if let Err(error @ (ToolError::Persistence(_) | ToolError::OutcomeUnknown(_))) =
                        &result
                    {
                        admission.fatal.get_or_insert_with(|| error.clone());
                        scope.cancellation.cancel();
                    }
                }
                // The effect is settled even if JS no longer observes its promise.
                let result = result
                    .map_err(|error| {
                        CellDiagnostic::new(CellDiagnosticKind::ToolFailure, error.to_string())
                    })
                    .and_then(|value| {
                        if serde_json::to_vec(&value).unwrap().len() > scope.limits.max_value_bytes
                        {
                            Err(CellDiagnostic::limit("tool output byte limit exceeded"))
                        } else {
                            Ok(value)
                        }
                    });
                let _ = sender.send(result);
            },
            &self.host,
        );
        Ok(receiver)
    }
}

// Synchronous admission before returning the future is intentional: even an
// unawaited JS call immediately becomes tracked host work.
#[op2]
#[serde]
fn op_maka_tool(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[serde] input: serde_json::Value,
) -> impl Future<Output = serde_json::Value> {
    let scope = state.borrow().borrow::<Arc<ToolScope>>().clone();
    let receiver = scope.start(name, input);
    async move {
        let result = match receiver {
            Ok(receiver) => receiver.await.unwrap_or_else(|_| {
                Err(CellDiagnostic::new(
                    CellDiagnosticKind::ExecutionError,
                    "tool outcome unavailable",
                ))
            }),
            Err(error) => Err(error),
        };
        match result {
            Ok(value) => serde_json::json!({"ok": true, "value": value}),
            Err(error) => serde_json::json!({"ok": false, "error": error}),
        }
    }
}

deno_core::extension!(maka_code, ops = [op_maka_tool]);
