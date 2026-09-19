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
    workhub::{ActInput, Proposal, RoutingProposal, SelectionInput},
};
use maka_runtime::{
    artifact::content_digest,
    capability::{FormResult, FormValue},
    event::Invocation,
    interaction::InteractionOutcome,
};
pub(crate) mod offer;

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
