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

use super::control::failure;
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    session::WorkspaceProjection,
    workhub::{ActInput, Proposal, RoutingProposal, SelectionInput, SelectionResult},
};
use maka_runtime::{
    artifact::content_digest,
    capability::{FormResult, FormValue},
    event::Invocation,
    interaction::{InteractionOutcome, InteractionRecord},
};
pub(crate) mod offer;

pub(crate) struct Source {
    pub invocation: Invocation,
    pub request_id: String,
    pub form: Option<InteractionRecord>,
}

impl super::Control {
    pub(crate) async fn select(
        &self,
        input: SelectionInput,
    ) -> Result<SelectionResult, OperationError> {
        self.select_cancellable(input, tokio_util::sync::CancellationToken::new())
            .await
    }

    pub(crate) async fn select_cancellable(
        &self,
        input: SelectionInput,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<SelectionResult, OperationError> {
        let _call = self
            .caller
            .admit()
            .map_err(|error| failure(Code::OperationUnavailable, error.to_string()))?;
        let stopping = self
            .caller
            .stopping()
            .map_err(|error| failure(Code::OperationUnavailable, error.to_string()))?
            .child_token();
        let source = self
            .commands
            .selection(self.caller.clone(), input.clone())
            .await?;
        let form = match source.form {
            Some(form) => form,
            None => {
                let page = self.candidates().await?.result;
                let request = offer::build(&page, &input)?;
                self.commands
                    .offer_selection(
                        self.caller.clone(),
                        input.clone(),
                        source.invocation.clone(),
                        request,
                    )
                    .await?
            }
        };
        let wait = self
            .commands
            .wait_selection(form.request_id, stopping.clone());
        tokio::pin!(wait);
        let outcome = tokio::select! {
            result = &mut wait => result?,
            _ = cancellation.cancelled() => { stopping.cancel(); wait.await? }
        };
        let Some(selection) = interpret(input, source.invocation, form.created_at, outcome)? else {
            return Ok(SelectionResult::Cancelled);
        };
        let result = self
            .delegate(selection.input, Some(selection.target))
            .await?;
        Ok(SelectionResult::Delegated { result })
    }
}

pub(crate) fn request_id(
    input: &SelectionInput,
    invocation: &Invocation,
) -> Result<String, OperationError> {
    let bytes = serde_json::to_vec(&("workhub.selection.v1", input, invocation))
        .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
    Ok(format!("whf_{}", &content_digest(&bytes)[7..]))
}

#[derive(Clone)]
pub(crate) struct SelectedTarget {
    pub invocation: Invocation,
    pub candidate_ref: String,
    pub session_id: String,
    pub workspace_digest: String,
    pub created_at: u64,
}

pub(crate) fn workspace_digest(workspace: &WorkspaceProjection) -> String {
    // This closed struct contains only strings and serializable enum variants.
    content_digest(&serde_json::to_vec(workspace).expect("workspace serialization"))
}

pub(crate) struct Selection {
    pub input: ActInput,
    pub target: SelectedTarget,
}

pub(crate) fn interpret(
    input: SelectionInput,
    invocation: Invocation,
    created_at: u64,
    outcome: InteractionOutcome,
) -> Result<Option<Selection>, OperationError> {
    let values = match outcome {
        InteractionOutcome::FormAnswer {
            result: FormResult::Accept { values },
            ..
        } => values,
        InteractionOutcome::FormAnswer { .. } | InteractionOutcome::Closure { .. } => {
            return Ok(None);
        }
        _ => {
            return Err(failure(
                Code::InternalFailure,
                "Target choice has an invalid outcome",
            ));
        }
    };
    let Some(FormValue::String(value)) = values.get("target") else {
        return Err(conflict("Target choice has no selected target"));
    };
    let (candidate_ref, session_id, workspace_digest): (String, String, String) =
        serde_json::from_str(value)
            .map_err(|_| conflict("Target choice has an invalid binding"))?;
    if !input.candidate_refs.contains(&candidate_ref) {
        return Err(conflict("Target choice was not offered by this request"));
    }
    let selected = SelectedTarget {
        invocation,
        candidate_ref: candidate_ref.clone(),
        session_id,
        workspace_digest,
        created_at,
    };
    Ok(Some(Selection {
        input: ActInput {
            turn_id: input.turn_id,
            action_id: input.action_id,
            proposal: Proposal::Route(RoutingProposal::DelegateExisting { candidate_ref }),
            candidate_set_id: Some(input.candidate_set_id),
            create: None,
            new_work_defaults: None,
            delegation_text: Some(input.delegation_text),
        },
        target: selected,
    }))
}

fn conflict(reason: &str) -> OperationError {
    failure(Code::OperationConflict, reason)
}
