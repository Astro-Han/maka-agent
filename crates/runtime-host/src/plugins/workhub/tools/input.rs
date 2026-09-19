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

use maka_protocol::workhub::{ActInput, LinkedProposal, LinkedTarget, Proposal, RoutingProposal};
use maka_runtime::{tools::ToolError, workhub::ActionId};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    pub request: Task,
}

#[derive(Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Task {
    Candidates,
    SelectAndDelegate {
        candidate_set_id: String,
        candidate_refs: Vec<String>,
        text: String,
    },
    DelegateExisting {
        candidate_set_id: String,
        candidate_ref: String,
        text: String,
    },
    CreateNew {
        title: String,
        text: String,
    },
    Correct {
        replaces_action_id: ActionId,
        candidate_set_id: Option<String>,
        target: RoutingProposal,
        text: String,
    },
    Stop {
        target_session_id: String,
    },
    Resume {
        target_session_id: String,
        resumes_action_id: ActionId,
    },
}
impl Task {
    pub(super) fn creates(&self) -> bool {
        matches!(
            self,
            Self::CreateNew { .. }
                | Self::Correct {
                    target: RoutingProposal::CreateNew { .. },
                    ..
                }
        )
    }
    pub(super) fn action(
        self,
        turn_id: String,
        action_id: ActionId,
        desktop: Option<super::desktop::Creation>,
    ) -> Result<ActInput, ToolError> {
        let (proposal, candidate_set_id, delegation_text) = match self {
            Self::DelegateExisting {
                candidate_set_id,
                candidate_ref,
                text,
            } => (
                Proposal::Route(RoutingProposal::DelegateExisting { candidate_ref }),
                Some(candidate_set_id),
                Some(text),
            ),
            Self::CreateNew { title, text } => (
                Proposal::Route(RoutingProposal::CreateNew { title }),
                None,
                Some(text),
            ),
            Self::Correct {
                replaces_action_id,
                candidate_set_id,
                target,
                text,
            } => (
                Proposal::Linked(LinkedProposal::Correct {
                    replaces_action_id,
                    target,
                }),
                candidate_set_id,
                Some(text),
            ),
            Self::Stop { target_session_id } => (
                Proposal::Linked(LinkedProposal::Stop {
                    expects: LinkedTarget { target_session_id },
                }),
                None,
                None,
            ),
            Self::Resume {
                target_session_id,
                resumes_action_id,
            } => (
                Proposal::Linked(LinkedProposal::Resume {
                    resumes_action_id,
                    expects: LinkedTarget { target_session_id },
                }),
                None,
                None,
            ),
            Self::Candidates | Self::SelectAndDelegate { .. } => {
                return Err(super::failed("not an action"));
            }
        };
        let (create, new_work_defaults) = desktop
            .map(|desktop| {
                (
                    Some(maka_protocol::workhub::CreateContext {
                        workspace: desktop.workspace,
                    }),
                    Some(desktop.defaults),
                )
            })
            .unwrap_or_default();
        let input = ActInput {
            turn_id,
            action_id,
            proposal,
            candidate_set_id,
            create,
            new_work_defaults,
            delegation_text,
        };
        // Apply the same closed wire bounds as the legacy entry point.
        maka_protocol::workhub::decode_act(&serde_json::to_value(input).map_err(super::failed)?)
            .map_err(super::failed)
    }
}
