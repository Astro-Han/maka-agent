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
use crate::session::SessionConfiguration;
use maka_event_log::sessions::SessionRecord;
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    session::{BehaviorId, CollaborationMode},
    workhub::{Candidate, CandidatesResult},
};
use maka_runtime::{artifact::content_digest, workhub::COORDINATION_SESSION_ID};

pub(crate) struct Candidates {
    pub result: CandidatesResult,
    pub records: Vec<SessionRecord<SessionConfiguration>>,
}

/// Candidate references bind this visible set, not the changing global catalog.
impl super::Control {
    pub(crate) async fn candidates(&self) -> Result<Candidates, OperationError> {
        let _call = self
            .caller
            .admit()
            .map_err(|error| failure(Code::OperationUnavailable, error.to_string()))?;
        let records = self.commands.candidates(eligible).await?;
        let mut candidates = records
            .iter()
            .map(|candidate| {
                let record = &candidate.session;
                let updated_at = maka_event_log::workhub::activity_at(record);
                let projection = crate::session::catalog_projection(record.clone());
                Candidate {
                    candidate_ref: String::new(),
                    session_id: record.id.clone(),
                    session_name: projection.name,
                    workspace: projection.workspace,
                    state: projection.status,
                    updated_at,
                    latest_delegation_action_id: candidate.latest_delegation_action_id.clone(),
                }
            })
            .collect::<Vec<_>>();
        let records = records
            .into_iter()
            .map(|candidate| candidate.session)
            .collect::<Vec<_>>();
        let surface = candidates
            .iter()
            .zip(records.iter().map(|record| &record.configuration))
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&surface)
            .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
        let candidate_set_id = content_digest(&bytes);
        for candidate in &mut candidates {
            let digest = content_digest(
                format!("{}\0{}", candidate_set_id, candidate.session_id).as_bytes(),
            );
            candidate.candidate_ref = format!("whc_{}", &digest[7..55]);
        }
        Ok(Candidates {
            result: CandidatesResult {
                candidate_set_id,
                candidates,
            },
            records,
        })
    }
}

pub(crate) fn eligible(id: &str, config: &SessionConfiguration) -> bool {
    id != COORDINATION_SESSION_ID
        && config.tool_profile.is_none()
        && config.collaboration_mode == CollaborationMode::Agent
        && config.orchestration_mode == BehaviorId::default()
        && !config
            .labels
            .iter()
            .any(|label| label == "mode:side_conversation")
}
