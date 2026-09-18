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

//! Startup reconciliation of abandoned invocations, never execution replay.

use maka_event_log::EventLog;
use maka_runtime::event::{Fact, InvocationOutcome, ModelInterruption, RuntimeEvent};
use maka_runtime::tool_call::ToolRejection;

use crate::RunError;

/// Requires exclusive Host Root ownership and no live model/tool workers.
/// Call before accepting connections or constructing execution workers. This
/// is not a live repair API: absence of T1 proves no effect only after the old
/// owner has exited. Errors fail host startup closed; retry requires reopening.
/// No model or tool executor is called, including for unfinished admissions.
pub async fn recover(log: &EventLog) -> Result<usize, RunError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| RunError::Internal(error.to_string()))?
        .as_millis();
    log.close_abandoned_interactions(
        u64::try_from(now).map_err(|error| RunError::Internal(error.to_string()))?,
    )
    .await?;
    let openings = log.unfinished_invocations(10_000).await?;
    for invocation in &openings {
        let state = log
            .invocation_recovery(invocation, 10_000, 8 * 1024 * 1024)
            .await?;
        let append = |fact| {
            let event = RuntimeEvent::new(invocation.clone(), fact);
            async move {
                let write = maka_runtime::event::EventWrite::plain(event)?;
                log.append(&write).await.map_err(RunError::from)
            }
        };
        for step_id in state.unfinished_model_steps {
            append(Fact::ModelInterrupted {
                step_id,
                status: ModelInterruption::Failed,
            })
            .await?;
        }
        for call in state.undispatched_calls {
            // A refusal closes the accepted call without claiming dispatch.
            append(Fact::ToolRejected {
                operation_id: call.operation_id,
                call: call.identity,
                name: call.name,
                input: call.input,
                reason: ToolRejection::Cancelled,
            })
            .await?;
        }
        append(Fact::InvocationEnded {
            outcome: InvocationOutcome::Failed {
                class: if state.uncertain_operations.is_empty() && !state.unfinished_executor {
                    "host_interrupted"
                } else {
                    "outcome_unknown"
                }
                .into(),
                message: None,
            },
        })
        .await?;
    }
    Ok(openings.len())
}
