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

use super::{Result, stored};
use crate::server::Host;
use maka_event_log::sessions::RemoveFamilyResult;
use maka_protocol::{OperationErrorCode as Code, session::*};

pub(super) async fn remove(host: &Host, input: SessionRemoveInput) -> Result<SessionRemoveResult> {
    let gate = host.interactions.own_admission().await;
    crate::session::require_unmanaged(&host.log, &input.session_id, Code::OperationConflict)
        .await?;
    let result = host
        .executions
        .remove_family(
            input.session_id.clone(),
            input.expected_revision,
            maka_event_log::sessions::RemovalAuthority::Unmanaged,
            gate,
        )
        .await?;
    Ok(match result {
        RemoveFamilyResult::Accepted(plan) => SessionRemoveResult::Removed {
            session_id: input.session_id,
            archived_subtask_count: Some(plan.archived_subtask_count),
        },
        RemoveFamilyResult::RevisionConflict { expected, actual } => {
            SessionRemoveResult::RevisionConflict {
                expected_revision: expected,
                actual_revision: actual,
            }
        }
    })
}

pub(super) async fn preview(
    host: &Host,
    input: SessionRemovePreviewInput,
) -> Result<SessionRemovePreviewResult> {
    crate::session::require_unmanaged(&host.log, &input.session_id, Code::OperationConflict)
        .await?;
    let plan = host
        .log
        .preview_session_removal(&input.session_id)
        .await
        .map_err(stored)?;
    Ok(SessionRemovePreviewResult {
        archivable_subtask_count: plan.archived_subtask_count,
    })
}

pub(super) async fn query(
    host: &Host,
    input: SessionRemoveQueryInput,
) -> Result<SessionRemoveQueryResult> {
    crate::session::require_unmanaged(&host.log, &input.session_id, Code::OperationConflict)
        .await?;
    Ok(
        match host
            .log
            .session_removal_receipt(&input.session_id)
            .await
            .map_err(stored)?
        {
            Some(plan) => SessionRemoveQueryResult::Removed {
                session_id: input.session_id,
                archived_subtask_count: plan.archived_subtask_count,
            },
            None => SessionRemoveQueryResult::Missing,
        },
    )
}
