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

use super::{Code, Host, OperationError, failure, sessions};
use crate::session::{PreparedSession, SessionConfiguration};
use maka_protocol::{
    session::*,
    workhub::{ActInput, Proposal, RoutingProposal},
};
use maka_runtime::workhub::{
    CreateSpec, DelegationDescription, DelegationKind, created_session_id,
};

pub(super) enum Target {
    Existing {
        id: String,
        name: String,
        revision: u64,
        configuration_digest: String,
    },
    Created {
        id: String,
        configuration: Box<SessionConfiguration>,
        spec: CreateSpec,
    },
}
impl Target {
    pub(super) fn description(&self) -> DelegationDescription {
        match self {
            Self::Existing { name, .. } => DelegationDescription::Existing { name: name.clone() },
            Self::Created {
                spec,
                configuration,
                ..
            } => DelegationDescription::Created {
                name: configuration.name.clone(),
                spec: spec.clone(),
            },
        }
    }
    pub(super) fn id(&self) -> &str {
        match self {
            Self::Existing { id, .. } | Self::Created { id, .. } => id,
        }
    }
    pub(super) fn revision(&self) -> u64 {
        match self {
            Self::Existing { revision, .. } => *revision,
            Self::Created { .. } => 1,
        }
    }
    pub(super) fn kind(&self) -> DelegationKind {
        match self {
            Self::Existing { .. } => DelegationKind::Existing,
            Self::Created { .. } => DelegationKind::Created,
        }
    }
}

pub(super) async fn prepare(
    host: &std::sync::Arc<Host>,
    input: &ActInput,
    selected: Option<&super::super::selection::SelectedTarget>,
) -> Result<Target, OperationError> {
    match &input.proposal {
        Proposal::Route(RoutingProposal::DelegateExisting { candidate_ref }) => {
            if let Some(selected) = selected {
                let now = crate::server::configuration::now()
                    .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
                if now.saturating_sub(selected.created_at) > 600_000 {
                    return Err(failure(
                        Code::CandidateSetStale,
                        "Target choice expired; discover candidates and ask again",
                    ));
                }
                let record = super::super::candidates::target(host, &selected.session_id)
                    .await?
                    .filter(|record| {
                        selected.candidate_ref == *candidate_ref
                            && selected.workspace_digest
                                == super::super::selection::workspace_digest(
                                    &record.configuration.workspace,
                                )
                    })
                    .ok_or_else(|| {
                        failure(
                            Code::CandidateSetStale,
                            "Selected target is no longer eligible in its offered workspace",
                        )
                    })?;
                return Ok(Target::Existing {
                    id: record.id,
                    name: record.configuration.name,
                    revision: record.revision,
                    configuration_digest: record.configuration_digest,
                });
            }
            let candidates = super::super::candidates::query(host).await?;
            if input.candidate_set_id.as_ref() != Some(&candidates.result.candidate_set_id) {
                return Err(failure(
                    Code::CandidateSetStale,
                    "WorkHub candidate set changed",
                ));
            }
            candidates
                .result
                .candidates
                .iter()
                .zip(candidates.records)
                .find_map(|(candidate, record)| {
                    (&candidate.candidate_ref == candidate_ref).then_some(Target::Existing {
                        id: record.id,
                        name: record.configuration.name,
                        revision: record.revision,
                        configuration_digest: record.configuration_digest,
                    })
                })
                .ok_or_else(|| {
                    failure(
                        Code::CandidateSetStale,
                        "WorkHub candidate is no longer eligible",
                    )
                })
        }
        Proposal::Route(RoutingProposal::CreateNew { title }) => {
            let id = created_session_id(&input.action_id);
            let context = input.create.as_ref().ok_or_else(|| {
                failure(
                    Code::OperationConflict,
                    "WorkHub creation context is missing",
                )
            })?;
            let model_target = input
                .new_work_defaults
                .as_ref()
                .and_then(|defaults| defaults.model.as_ref())
                .map_or(SessionModelTarget::Default, |model| {
                    SessionModelTarget::Explicit {
                        connection_id: model.llm_connection_id.clone(),
                        connection_slug: model.llm_connection_slug.clone(),
                        model: model.model.clone(),
                    }
                });
            let prepared = PreparedSession::new(SessionCreateInput {
                session_id: id.clone(),
                workspace: context.workspace.clone(),
                model_target,
                mode: None,
                name: Some(title.clone()),
                labels: None,
                thinking_level: None,
                tool_profile: None,
                permission_mode: input
                    .new_work_defaults
                    .as_ref()
                    .and_then(|defaults| defaults.permission_mode),
                collaboration_mode: Some(CollaborationMode::Agent),
                orchestration_mode: Some(OrchestrationMode::Default),
            })
            .map_err(|error| failure(Code::OperationConflict, error.to_string()))?;
            let workspace = sessions::workspace::resolve(host, prepared.workspace())
                .await
                .map_err(context_error)?;
            let configuration =
                sessions::create::resolve(&host.configuration, prepared, None, workspace)
                    .await
                    .map_err(context_error)?;
            super::super::super::projects::record_usage(host, &configuration.workspace).await?;
            Ok(Target::Created {
                id,
                configuration: Box::new(configuration),
                spec: CreateSpec {
                    title: title.clone(),
                    workspace: context.workspace.clone(),
                    defaults: input.new_work_defaults.clone(),
                },
            })
        }
        Proposal::Linked(_) => Err(failure(
            Code::OperationUnavailable,
            "This WorkHub action is not installed",
        )),
    }
}

fn context_error(mut error: OperationError) -> OperationError {
    // Valid action syntax can still refer to a stale model or workspace.
    if error.code == Code::InvalidRequest {
        error.code = Code::OperationConflict;
    }
    error
}
