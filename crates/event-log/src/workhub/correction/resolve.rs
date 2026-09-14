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

use super::{Assignment, CorrectionIntent, Delegation, StoreError, control, invalid};
use maka_runtime::{
    event::{Fact, RuntimeEvent},
    session_event::{SessionEvent, SessionFact},
    workhub::DelegationKind,
};
use sqlx::SqliteConnection;

pub(super) async fn retired(
    tx: &mut SqliteConnection,
    intent: &CorrectionIntent,
) -> Result<(), StoreError> {
    let Some(owner) = &intent.owner else {
        return Ok(());
    };
    let json: Option<Option<String>> = sqlx::query_scalar(
        "SELECT CASE WHEN length(CAST(event_json AS BLOB)) <= 1048576 THEN event_json END
         FROM runtime_events WHERE invocation_id = ? AND kind = 'invocation_ended'",
    )
    .bind(&owner.invocation_id)
    .fetch_optional(tx)
    .await?;
    let terminal: RuntimeEvent = serde_json::from_str(
        &json
            .ok_or(StoreError::SessionBusy)?
            .ok_or(StoreError::PrefixTooLarge)?,
    )?;
    if terminal.invocation != *owner || !matches!(terminal.fact, Fact::InvocationEnded { .. }) {
        return Err(invalid("WorkHub correction retirement owner changed"));
    }
    Ok(())
}

pub(super) async fn assign(
    tx: &mut SqliteConnection,
    intent: &CorrectionIntent,
    delegation: Delegation,
    configuration: Option<String>,
) -> Result<Assignment, StoreError> {
    let request = &intent.request;
    delegation.validate(&request.source).map_err(invalid)?;
    if delegation.action_id != request.action_id
        || delegation.request_fingerprint != request.request_fingerprint
        || delegation.source_message_event_id != request.source_message_event_id
        || delegation.target.session_id != request.target.session_id()
        || delegation.delegation_text != request.delegation_text
        || delegation.kind != request.target.kind()
        || delegation.description.as_ref().is_none_or(|description| {
            match (description, &request.target.description()) {
                (
                    maka_runtime::workhub::DelegationDescription::Existing { .. },
                    maka_runtime::workhub::DelegationDescription::Existing { .. },
                ) => false,
                (
                    maka_runtime::workhub::DelegationDescription::Created { spec, .. },
                    maka_runtime::workhub::DelegationDescription::Created {
                        spec: expected, ..
                    },
                ) => spec != expected,
                _ => true,
            }
        })
    {
        return Err(invalid(
            "WorkHub correction assignment changed its admitted intent",
        ));
    }
    let event = SessionEvent::workhub(
        request.source.turn_id.clone(),
        SessionFact::WorkhubDelegated {
            coordinator: request.source.clone(),
            delegation: Box::new(delegation.clone()),
            replaces_action_id: request.replaces_action_id.clone(),
        },
    );
    match (delegation.kind, configuration) {
        (DelegationKind::Created, Some(configuration)) => {
            let now = event
                .recorded_at
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid("invalid WorkHub creation time"))?
                .as_millis()
                .try_into()
                .map_err(|_| invalid("invalid WorkHub creation time"))?;
            crate::sessions::insert(
                tx,
                &delegation.target.session_id,
                &format!("workhub.create:{}", delegation.request_fingerprint),
                &configuration,
                now,
            )
            .await?;
        }
        (DelegationKind::Existing, None) => {
            let current: Option<(bool, String)> = sqlx::query_as(
                "SELECT archived, json_extract(configuration, '$.workspace') FROM session_control WHERE id = ?"
            ).bind(&delegation.target.session_id).fetch_optional(&mut *tx).await?;
            let (archived, workspace) = current.ok_or(StoreError::SessionNotFound)?;
            if archived
                || Some(maka_runtime::artifact::content_digest(workspace.as_bytes())).as_deref()
                    != request.target.workspace_digest()
            {
                return Err(invalid("WorkHub replacement target workspace changed"));
            }
        }
        _ => {
            return Err(invalid(
                "WorkHub replacement creation configuration mismatch",
            ));
        }
    }
    super::super::deliver(tx, &request.source, &delegation, event.recorded_at).await?;
    let sequence = control::append(tx, &event).await?;
    Ok(Assignment {
        sequence,
        id: event.id,
        recorded_at: event.recorded_at,
        coordinator: request.source.clone(),
        delegation: Box::new(delegation),
    })
}
