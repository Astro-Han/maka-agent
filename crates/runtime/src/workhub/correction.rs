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

use super::{COORDINATION_SESSION_ID, CreateSpec, DelegationDescription, DelegationKind};
use crate::event::Invocation;
use crate::workhub::ActionId;
use serde::{Deserialize, Serialize};

/// Stable replacement choice. Candidate references never survive admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CorrectionTarget {
    Existing {
        session_id: String,
        name: String,
        workspace_digest: String,
    },
    Created {
        session_id: String,
        name: String,
        spec: CreateSpec,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionRequest {
    pub action_id: ActionId,
    pub request_fingerprint: String,
    pub source: Invocation,
    pub source_message_event_id: String,
    pub replaces_action_id: ActionId,
    pub target: CorrectionTarget,
    pub delegation_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionIntent {
    pub request: CorrectionRequest,
    /// None proves the old pending Message was cancelled, already cancelled,
    /// or shared. A shared Run is never owned by this operation.
    pub owner: Option<Invocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionAbort {
    TargetUnavailable,
    TargetWaitingForUser,
}

impl CorrectionTarget {
    pub fn kind(&self) -> DelegationKind {
        match self {
            Self::Existing { .. } => DelegationKind::Existing,
            Self::Created { .. } => DelegationKind::Created,
        }
    }
    pub fn create(&self) -> Option<&CreateSpec> {
        match self {
            Self::Created { spec, .. } => Some(spec),
            _ => None,
        }
    }
    pub fn session_id(&self) -> &str {
        match self {
            Self::Existing { session_id, .. } | Self::Created { session_id, .. } => session_id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Self::Existing { name, .. } | Self::Created { name, .. } => name,
        }
    }
    pub fn workspace_digest(&self) -> Option<&str> {
        match self {
            Self::Existing {
                workspace_digest, ..
            } => Some(workspace_digest),
            _ => None,
        }
    }
    pub fn description(&self) -> DelegationDescription {
        match self {
            Self::Existing { name, .. } => DelegationDescription::Existing { name: name.clone() },
            Self::Created { name, spec, .. } => DelegationDescription::Created {
                name: name.clone(),
                spec: spec.clone(),
            },
        }
    }
}

impl CorrectionRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        for id in [
            &self.source_message_event_id,
            &self.source.session_id,
            &self.source.turn_id,
            &self.source.run_id,
            &self.source.invocation_id,
            self.target.session_id(),
        ] {
            crate::interaction::entity_id(id)?;
        }
        self.target.description().validate(self.target.kind())?;
        if self
            .target
            .workspace_digest()
            .is_some_and(|digest| !crate::archive::valid_projection_digest(digest))
        {
            return Err("invalid WorkHub correction workspace basis");
        }
        if self.source.session_id != COORDINATION_SESSION_ID
            || self.target.session_id() == COORDINATION_SESSION_ID
            || self.action_id == self.replaces_action_id
            || !crate::archive::valid_projection_digest(&self.request_fingerprint)
            || self.delegation_text.trim().is_empty()
            || self.delegation_text.len() > 48 * 1024
            || (self.target.kind() == DelegationKind::Created
                && self.target.session_id() != super::created_session_id(&self.action_id))
        {
            return Err("invalid WorkHub correction authority");
        }
        Ok(())
    }
}

impl CorrectionIntent {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.request.validate()?;
        if let Some(owner) = &self.owner {
            for id in [
                &owner.session_id,
                &owner.turn_id,
                &owner.run_id,
                &owner.invocation_id,
            ] {
                crate::interaction::entity_id(id)?;
            }
            if owner.session_id == COORDINATION_SESSION_ID
                || owner.session_id == self.request.target.session_id()
            {
                return Err("invalid WorkHub correction owner");
            }
        }
        Ok(())
    }
}

pub fn correction_abort_source(action_id: &ActionId) -> String {
    let digest = crate::artifact::content_digest(action_id.as_str().as_bytes());
    format!("workhub.replacement_stop.{}", &digest[7..55])
}
