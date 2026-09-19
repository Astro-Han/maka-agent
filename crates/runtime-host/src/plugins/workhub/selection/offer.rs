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

use super::{Code, OperationError, SelectionInput, workspace_digest};
use crate::plugins::workhub::control::failure;
use maka_protocol::workhub::CandidatesResult;
use maka_runtime::{
    capability::{FormField, FormFieldSpec, FormOption, FormRequester},
    interaction::InteractionRequest,
};

pub(crate) fn build(
    page: &CandidatesResult,
    input: &SelectionInput,
) -> Result<InteractionRequest, OperationError> {
    if page.candidate_set_id != input.candidate_set_id {
        return Err(failure(
            Code::CandidateSetStale,
            "Discover fresh candidates before requesting a target choice",
        ));
    }
    let options = input
        .candidate_refs
        .iter()
        .enumerate()
        .map(|(index, reference)| {
            let candidate = page
                .candidates
                .iter()
                .find(|candidate| &candidate.candidate_ref == reference)
                .ok_or_else(|| {
                    super::conflict("Target choice contains an unavailable candidate")
                })?;
            let display = format!(
                "{} — {}",
                candidate.session_name, candidate.workspace.host_cwd
            );
            let mut label = format!("{}. ", index + 1);
            for character in display.chars() {
                if label.len() + character.len_utf8() > 190 {
                    break;
                }
                label.push(character);
            }
            let value = serde_json::to_string(&(
                reference,
                &candidate.session_id,
                workspace_digest(&candidate.workspace),
            ))
            .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
            Ok(FormOption { value, label })
        })
        .collect::<Result<Vec<_>, OperationError>>()?;
    Ok(InteractionRequest::Form {
        tool_use_id: input.action_id.to_string(),
        message: "Choose the work to continue".into(),
        requester: FormRequester {
            name: "WorkHub".into(),
            source: None,
        },
        fields: vec![FormField {
            name: "target".into(),
            label: "Work / Workspace".into(),
            required: true,
            description: None,
            spec: FormFieldSpec::SingleSelect {
                options,
                default: None,
            },
        }],
    })
}
