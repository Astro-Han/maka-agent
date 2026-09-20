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

use super::{CorrectionIntent, CorrectionRequest, StoreError, assignment, invalid};
use crate::message_resolution::{MessageExecution, owner};
use maka_runtime::event::Invocation;
use sqlx::SqliteConnection;

pub(super) async fn apply(
    tx: &mut SqliteConnection,
    request: CorrectionRequest,
    target_revision: Option<u64>,
    target_owner: Option<&Invocation>,
) -> Result<CorrectionIntent, StoreError> {
    if super::super::actions::read(tx, &request.action_id)
        .await?
        .is_some()
        || super::super::stop::read(tx, &request.action_id)
            .await?
            .is_some()
    {
        return Err(invalid(
            "WorkHub action already belongs to another operation",
        ));
    }
    let content = super::super::actions::require_coordinator(
        tx,
        &request.source,
        Some(&request.source_message_event_id),
    )
    .await?;
    // The target Message envelope has the same fixed labels as Delegation::message.
    if content.text_bytes()
        + request.delegation_text.len()
        + "User request:\n\n\nDelegated task:\n".len()
        > 64 * 1024
    {
        return Err(invalid("delegated message exceeds durable capacity"));
    }
    let replaced = assignment::read(tx, &request.replaces_action_id)
        .await?
        .ok_or_else(|| invalid("WorkHub correction delegation is missing"))?;
    let old = &replaced.delegation;
    if old.target.session_id == request.target.session_id() {
        return Err(invalid(
            "WorkHub correction requires a different target Session",
        ));
    }
    let latest: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(a.event_json, '$.fact.delegation.action_id') FROM workhub_assignments a
         WHERE json_extract(a.event_json, '$.fact.delegation.target.session_id') = ?
         AND NOT EXISTS(SELECT 1 FROM workhub_corrections c
             WHERE c.replaces_action_id = json_extract(a.event_json, '$.fact.delegation.action_id')
               AND c.resolution_kind IS NOT NULL)
         AND NOT EXISTS(SELECT 1 FROM workhub_stops s
             WHERE s.delegation_action_id = json_extract(a.event_json, '$.fact.delegation.action_id')
               AND json_extract(s.resolution_json, '$.outcome') != 'not_owned')
         ORDER BY a.sequence DESC LIMIT 1"
    ).bind(&old.target.session_id).fetch_optional(&mut *tx).await?;
    if latest.as_deref() != Some(old.action_id.as_str()) {
        return Err(invalid(
            "WorkHub correction requires the latest active association",
        ));
    }
    assignment::require_unclaimed(tx, &old.action_id).await?;
    match request.target.kind() {
        maka_runtime::workhub::DelegationKind::Existing => {
            let current: Option<(i64, bool, String, String)> = sqlx::query_as(
                "SELECT revision, archived, json_extract(configuration, '$.name'),
                 json_extract(configuration, '$.workspace') FROM session_control WHERE id = ?",
            )
            .bind(request.target.session_id())
            .fetch_optional(&mut *tx)
            .await?;
            let (revision, archived, name, workspace) =
                current.ok_or(StoreError::SessionNotFound)?;
            if archived
                || Some(revision as u64) != target_revision
                || name != request.target.name()
                || Some(maka_runtime::artifact::content_digest(workspace.as_bytes())).as_deref()
                    != request.target.workspace_digest()
            {
                return Err(invalid(
                    "WorkHub correction candidate changed before admission",
                ));
            }
            super::super::candidates::require_available(
                tx,
                request.target.session_id(),
                target_owner,
            )
            .await?;
        }
        maka_runtime::workhub::DelegationKind::Created => {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM session_control WHERE id = ?)")
                    .bind(request.target.session_id())
                    .fetch_one(&mut *tx)
                    .await?;
            if exists || target_revision.is_some() || target_owner.is_some() {
                return Err(invalid("WorkHub correction creation identity is occupied"));
            }
        }
    }
    let work = owner::execution(tx, &old.target.session_id, &old.target_message_id()).await?;
    let owner = match work {
        MessageExecution::Pending => {
            sqlx::query("INSERT INTO message_cancellations(session_id, message_id, cancellation_id) VALUES (?, ?, ?)")
                .bind(&old.target.session_id).bind(old.target_message_id()).bind(request.action_id.as_str())
                .execute(&mut *tx).await?;
            sqlx::query("DELETE FROM message_admissions WHERE session_id = ? AND message_id = ?")
                .bind(&old.target.session_id)
                .bind(old.target_message_id())
                .execute(&mut *tx)
                .await?;
            crate::message_queue::bump(tx, &old.target.session_id).await?;
            None
        }
        MessageExecution::Cancelled | MessageExecution::Shared(_) => None,
        MessageExecution::Owned(boundary) => Some(boundary.invocation),
        MessageExecution::Missing => {
            return Err(invalid(
                "WorkHub delegation has no recoverable Message owner",
            ));
        }
    };
    Ok(CorrectionIntent {
        request,
        owner,
        preparation: None,
    })
}
