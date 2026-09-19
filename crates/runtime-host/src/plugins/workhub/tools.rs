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

use super::Control;
use input::{Input, Task};
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    workhub::{ActResult, LinkedProposal, Proposal, SelectionInput, SelectionResult},
};
use maka_runtime::{
    tool_call::ToolRejection,
    tools::{PreparationFuture, PreparedEffect, ToolCallContext, ToolError, ToolPreparer},
    workhub::{ActionId, COORDINATION_SESSION_ID},
};
use maka_tools::{ToolDefinition, ToolHandler, ToolNesting, ToolRegistration, ToolSemantics};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(crate) mod desktop;
mod input;
mod schema;

pub(crate) const NAME: &str = "workhub_tasks";

pub(crate) fn registration(control: Control) -> ToolRegistration {
    ToolRegistration {
        definition: ToolDefinition {
            name: NAME.into(),
            description: "Discover and coordinate tasks on this Host. The original user request and exact delegation determine authority; model arguments cannot replace either.".into(),
            input_schema: schema::input(),
        },
        nesting: ToolNesting::Nestable,
        semantics: ToolSemantics::ExclusiveStep,
        handler: ToolHandler::Prepared(Arc::new(Tasks(control))),
    }
}
struct Tasks(Control);
impl ToolPreparer for Tasks {
    fn names(&self) -> Vec<String> {
        vec![NAME.into()]
    }
    fn prepare(
        &self,
        name: String,
        input: Value,
        context: ToolCallContext,
        _: CancellationToken,
    ) -> PreparationFuture {
        let control = self.0.clone();
        Box::pin(async move {
            if name != NAME || context.invocation.session_id != COORDINATION_SESSION_ID {
                return Err(ToolRejection::Unavailable);
            }
            let input: Input =
                serde_json::from_value(input).map_err(|error| ToolRejection::InvalidInput {
                    message: error.to_string(),
                })?;
            let caller = control.caller.clone();
            Ok(PreparedEffect::new(move |cancellation| {
                Box::pin(async move {
                    if cancellation.is_cancelled() {
                        return Err(failed("WorkHub call cancelled"));
                    }
                    execute(control, input.request, context, cancellation)
                        .await
                        .map(Into::into)
                })
            })
            .guarded(move || caller.admit().map_err(failed)))
        })
    }
}
async fn execute(
    control: Control,
    task: Task,
    context: ToolCallContext,
    cancellation: CancellationToken,
) -> Result<Value, ToolError> {
    let action_id = ActionId::new(context.tool_use_id()).map_err(failed)?;
    if matches!(task, Task::Candidates) {
        return serde_json::to_value(control.candidates().await.map_err(operation_error)?.result)
            .map_err(failed);
    }
    if let Task::SelectAndDelegate {
        candidate_set_id,
        candidate_refs,
        text,
    } = task
    {
        let input = SelectionInput {
            turn_id: context.invocation.turn_id,
            action_id: action_id.clone(),
            candidate_set_id,
            candidate_refs,
            delegation_text: text,
        };
        let input =
            maka_protocol::workhub::decode_selection(&serde_json::to_value(input).map_err(failed)?)
                .map_err(failed)?;
        return match control
            .select_cancellable(input, cancellation)
            .await
            .map_err(operation_error)?
        {
            SelectionResult::Cancelled => Ok(serde_json::json!({"kind":"cancelled"})),
            SelectionResult::Delegated { result } => result_value(action_id, result),
        };
    }
    let desktop = if task.creates() {
        Some(desktop::creation(&control, context.clone(), cancellation.clone()).await?)
    } else {
        None
    };
    if cancellation.is_cancelled() {
        return Err(failed("WorkHub call cancelled"));
    }
    let input = task.action(
        context.invocation.turn_id.clone(),
        action_id.clone(),
        desktop,
    )?;
    let result = match &input.proposal {
        Proposal::Route(_) => control.delegate(input, None).await,
        Proposal::Linked(LinkedProposal::Correct { .. }) => control.correct(input).await,
        Proposal::Linked(LinkedProposal::Stop { .. }) => control.stop(input).await,
        Proposal::Linked(LinkedProposal::Resume { .. }) => {
            let connection = control
                .commands
                .client_connection(
                    control.caller.clone(),
                    context.invocation,
                    desktop::CONTROL_TOOL,
                )
                .await
                .map_err(operation_error)?;
            control.resume(input, connection).await
        }
    }
    .map_err(operation_error)?;
    result_value(action_id, result)
}
fn result_value(action_id: ActionId, result: ActResult) -> Result<Value, ToolError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Receipt {
        action_id: ActionId,
        #[serde(flatten)]
        result: ActResult,
    }
    serde_json::to_value(Receipt { action_id, result }).map_err(failed)
}
fn operation_error(error: OperationError) -> ToolError {
    match error.code {
        Code::CommitOutcomeUnknown => ToolError::OutcomeUnknown(error.message),
        Code::PersistenceFailed => ToolError::Persistence(error.message),
        _ => failed(error.message),
    }
}
fn failed(error: impl ToString) -> ToolError {
    ToolError::Failed(error.to_string())
}
