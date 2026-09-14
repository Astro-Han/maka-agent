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

use crate::{
    ProtocolError, Result, codec,
    session::{SessionStatus, WorkspaceProjection, WorkspaceTarget},
};
use maka_runtime::workhub::ActionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Candidate {
    pub candidate_ref: String,
    pub session_id: String,
    pub session_name: String,
    pub workspace: WorkspaceProjection,
    pub state: SessionStatus,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_delegation_action_id: Option<ActionId>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidatesResult {
    pub candidate_set_id: String,
    pub candidates: Vec<Candidate>,
}

pub fn decode_candidates(value: &Value) -> Result<CandidatesResult> {
    let result: CandidatesResult = crate::turn::decode(value)?;
    text(&result.candidate_set_id, 96)?;
    if result.candidates.len() > 32 {
        return Err(ProtocolError::invalid("Too many WorkHub candidates"));
    }
    for candidate in &result.candidates {
        crate::turn::entity(&candidate.candidate_ref)?;
        crate::turn::entity(&candidate.session_id)?;
        text(&candidate.session_name, 512)?;
        workspace(&candidate.workspace.target)?;
        workspace(&WorkspaceTarget::HostPath {
            path: candidate.workspace.host_cwd.clone(),
        })?;
        if let WorkspaceTarget::HostPath { path } = &candidate.workspace.target
            && path != &candidate.workspace.host_cwd
        {
            return Err(ProtocolError::invalid("Workspace path mismatch"));
        }
    }
    Ok(result)
}

pub(super) fn text(value: &str, max: usize) -> Result<()> {
    if value.is_empty() || value.len() > max {
        return Err(ProtocolError::invalid("Invalid WorkHub string"));
    }
    Ok(())
}

pub(super) fn workspace(target: &WorkspaceTarget) -> Result<()> {
    match target {
        WorkspaceTarget::Project { project_id } => crate::turn::entity(project_id),
        WorkspaceTarget::HostPath { path } => {
            text(path, 4096)?;
            if path.contains('\0') || !codec::absolute_host_path(path) {
                return Err(ProtocolError::invalid("Invalid WorkHub workspace path"));
            }
            Ok(())
        }
    }
}
