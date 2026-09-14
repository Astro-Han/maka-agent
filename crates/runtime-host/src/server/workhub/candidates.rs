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

use super::{Host, failure, sessions};
use crate::session::SessionConfiguration;
use maka_event_log::sessions::SessionRecord;
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    session::{CollaborationMode, OrchestrationMode},
    workhub::{Candidate, CandidatesResult},
};
use maka_runtime::{artifact::content_digest, workhub::COORDINATION_SESSION_ID};
use std::sync::Arc;

pub(in crate::server) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::PersistenceFailed,
    Code::InternalFailure,
];

pub(super) struct Candidates {
    pub result: CandidatesResult,
    pub records: Vec<SessionRecord<SessionConfiguration>>,
}

/// Candidate references bind this visible set, not the changing global catalog.
pub(super) async fn query(host: &Arc<Host>) -> Result<Candidates, OperationError> {
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let executions = host.executions.clone();
    let records = host
        .log
        .workhub_candidates(move |record| eligible(&executions, record))
        .await
        .map_err(sessions::stored)?;
    let mut candidates = records
        .iter()
        .map(|record| {
            let updated_at = maka_event_log::workhub::activity_at(record);
            let projection = sessions::projection::project(record.clone());
            Candidate {
                candidate_ref: String::new(),
                session_id: record.id.clone(),
                session_name: projection.name,
                workspace: projection.workspace,
                state: projection.status,
                updated_at,
                latest_delegation_action_id: None,
            }
        })
        .collect::<Vec<_>>();
    let surface = candidates
        .iter()
        .zip(records.iter().map(|record| &record.configuration))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&surface)
        .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
    let candidate_set_id = content_digest(&bytes);
    for candidate in &mut candidates {
        let digest =
            content_digest(format!("{}\0{}", candidate_set_id, candidate.session_id).as_bytes());
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

pub(super) async fn target(
    host: &Arc<Host>,
    id: &str,
) -> Result<Option<SessionRecord<SessionConfiguration>>, OperationError> {
    let executions = host.executions.clone();
    host.log
        .workhub_candidate(id, move |record| eligible(&executions, record))
        .await
        .map_err(sessions::stored)
}

fn eligible(
    executions: &crate::execution::Executions,
    record: &SessionRecord<SessionConfiguration>,
) -> bool {
    let config = &record.configuration;
    record.id != COORDINATION_SESSION_ID
        && !executions.has_active_session(&record.id)
        && config.tool_profile.is_none()
        && config.collaboration_mode == CollaborationMode::Agent
        && config.orchestration_mode == OrchestrationMode::Default
        && !config
            .labels
            .iter()
            .any(|label| label == "mode:side_conversation")
}
