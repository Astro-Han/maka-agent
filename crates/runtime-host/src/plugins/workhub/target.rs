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

use super::{
    candidates::Candidates,
    control::{Result, failure},
    selection::{SelectedTarget, workspace_digest},
};
use crate::session::SessionConfiguration;
use maka_event_log::sessions::SessionRecord;
use maka_protocol::{
    OperationErrorCode as Code,
    session::*,
    workhub::{ActInput, Proposal, RoutingProposal},
};
use maka_runtime::workhub::{
    CreateSpec, DelegationDescription, DelegationKind, created_session_id,
};

pub(crate) enum Target {
    Existing {
        id: String,
        name: String,
        revision: u64,
        configuration_digest: String,
        workspace_digest: String,
    },
    Created {
        id: String,
        configuration: Box<SessionConfiguration>,
        spec: CreateSpec,
    },
}
impl Target {
    pub(crate) fn description(&self) -> DelegationDescription {
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
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Existing { id, .. } | Self::Created { id, .. } => id,
        }
    }
    pub(crate) fn revision(&self) -> u64 {
        match self {
            Self::Existing { revision, .. } => *revision,
            Self::Created { .. } => 1,
        }
    }
    pub(crate) fn kind(&self) -> DelegationKind {
        match self {
            Self::Existing { .. } => DelegationKind::Existing,
            Self::Created { .. } => DelegationKind::Created,
        }
    }
}

pub(crate) fn route(input: &ActInput) -> Result<&RoutingProposal> {
    match &input.proposal {
        Proposal::Route(route) => Ok(route),
        Proposal::Linked(maka_protocol::workhub::LinkedProposal::Correct { target, .. }) => {
            Ok(target)
        }
        _ => Err(failure(
            Code::OperationConflict,
            "WorkHub operation has no routing target",
        )),
    }
}

fn existing(record: SessionRecord<SessionConfiguration>) -> Target {
    Target::Existing {
        workspace_digest: workspace_digest(&record.configuration.workspace),
        id: record.id,
        name: record.configuration.name,
        revision: record.revision,
        configuration_digest: record.configuration_digest,
    }
}

pub(crate) fn offered(input: &ActInput, reference: &str, candidates: Candidates) -> Result<Target> {
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
            (candidate.candidate_ref == reference).then(|| existing(record))
        })
        .ok_or_else(|| {
            failure(
                Code::CandidateSetStale,
                "WorkHub candidate is no longer eligible",
            )
        })
}

pub(crate) fn chosen(
    selected: &SelectedTarget,
    reference: &str,
    record: Option<SessionRecord<SessionConfiguration>>,
    now: u64,
) -> Result<Target> {
    if now.saturating_sub(selected.created_at) > 600_000 {
        return Err(failure(
            Code::CandidateSetStale,
            "Target choice expired; discover candidates and ask again",
        ));
    }
    record
        .filter(|record| {
            selected.candidate_ref == reference
                && selected.workspace_digest == workspace_digest(&record.configuration.workspace)
        })
        .map(existing)
        .ok_or_else(|| {
            failure(
                Code::CandidateSetStale,
                "Selected target is no longer eligible in its offered workspace",
            )
        })
}

pub(crate) fn creation(input: &ActInput, title: &str) -> Result<(SessionCreateInput, CreateSpec)> {
    let context = input.create.as_ref().ok_or_else(|| {
        failure(
            Code::OperationConflict,
            "WorkHub creation context is missing",
        )
    })?;
    let model_target = input
        .new_work_defaults
        .as_ref()
        .and_then(|defaults| defaults.model())
        .map_or(SessionModelTarget::Default, |model| {
            SessionModelTarget::Explicit {
                connection_id: model.llm_connection_id.clone(),
                connection_slug: model.llm_connection_slug.clone(),
                model: model.model.clone(),
            }
        });
    let target = match input
        .new_work_defaults
        .as_ref()
        .and_then(|defaults| defaults.execution.as_ref())
    {
        Some(maka_runtime::workhub::CreateExecution::Executor(executor_id)) => {
            SessionCreateTarget::Executor {
                executor_id: executor_id.clone(),
            }
        }
        _ => SessionCreateTarget::Model { model_target },
    };
    Ok((
        SessionCreateInput {
            session_id: created_session_id(&input.action_id),
            workspace: context.workspace.clone(),
            target,
            mode: None,
            name: Some(title.into()),
            labels: None,
            thinking_level: None,
            tool_profile: None,
            permission_mode: input
                .new_work_defaults
                .as_ref()
                .and_then(|defaults| defaults.permission_mode),
            collaboration_mode: Some(CollaborationMode::Agent),
            orchestration_mode: Some(BehaviorId::default()),
        },
        CreateSpec {
            title: title.into(),
            workspace: context.workspace.clone(),
            defaults: input.new_work_defaults.clone(),
        },
    ))
}
