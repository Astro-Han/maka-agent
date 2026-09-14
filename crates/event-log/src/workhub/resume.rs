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

use super::{actions, candidates, invalid};
use crate::{StoreError, message_resolution::MessageExecution, turns::InvocationState};
use maka_runtime::{
    event::{Fact, InvocationOutcome, RuntimeEvent},
    input::InvocationInput,
    workhub::{COORDINATION_SESSION_ID, ResumeOrigin, resumed_turn_id},
};
use sqlx::SqliteConnection;

pub(super) fn validate_record(event: &RuntimeEvent) -> Result<&ResumeOrigin, StoreError> {
    let (origin, target) = match &event.fact {
        Fact::WorkhubResumeObserved { resume, target } => {
            if event.invocation != resume.coordinator {
                return Err(invalid(
                    "WorkHub observation belongs to another coordinator",
                ));
            }
            (resume.as_ref(), target)
        }
        Fact::InvocationOpened {
            input:
                InvocationInput::Continuation {
                    workhub_resume: Some(origin),
                    request_fingerprint,
                    ..
                },
            ..
        } => {
            if event.invocation.turn_id != resumed_turn_id(&origin.action_id)
                || *request_fingerprint != origin.request_fingerprint
            {
                return Err(invalid("WorkHub continuation identity changed"));
            }
            (origin, &event.invocation)
        }
        _ => return Err(invalid("WorkHub action has no resume receipt")),
    };
    origin.validate().map_err(invalid)?;
    if target.session_id == COORDINATION_SESSION_ID {
        return Err(invalid("WorkHub cannot resume its coordination Session"));
    }
    for id in [
        &target.session_id,
        &target.turn_id,
        &target.run_id,
        &target.invocation_id,
    ] {
        crate::sessions::validate_id(id)?;
    }
    Ok(origin)
}

pub(super) async fn validate(
    tx: &mut SqliteConnection,
    event: &RuntimeEvent,
) -> Result<(), StoreError> {
    if !matches!(
        &event.fact,
        Fact::WorkhubResumeObserved { .. }
            | Fact::InvocationOpened {
                input: InvocationInput::Continuation {
                    workhub_resume: Some(_),
                    ..
                },
                ..
            }
    ) {
        return Ok(());
    }
    let origin = validate_record(event)?;
    actions::require_coordinator(tx, &origin.coordinator).await?;
    let stopped: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workhub_stops WHERE action_id = ?1
         OR (delegation_action_id = ?2 AND (resolution_json IS NULL
             OR json_extract(resolution_json, '$.outcome') != 'not_owned')))",
    )
    .bind(origin.action_id.as_str())
    .bind(origin.delegation_action_id.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if stopped {
        return Err(invalid("WorkHub resume conflicts with a stop claim"));
    }
    let target = match &event.fact {
        Fact::WorkhubResumeObserved { target, .. } => target,
        _ => &event.invocation,
    };
    let archived: Option<bool> =
        sqlx::query_scalar("SELECT archived FROM session_control WHERE id = ?")
            .bind(&target.session_id)
            .fetch_optional(&mut *tx)
            .await?;
    match archived {
        None => return Err(StoreError::SessionNotFound),
        Some(true) => return Err(invalid("WorkHub resume target is archived")),
        Some(false) => {}
    }
    super::assignment::require_unclaimed(tx, &origin.delegation_action_id).await?;
    let delegated = super::assignment::read(tx, &origin.delegation_action_id)
        .await?
        .ok_or_else(|| invalid("WorkHub resume delegation is missing"))?;
    let delegation = delegated.delegation;
    if delegation.target.session_id != target.session_id {
        return Err(invalid("WorkHub resume target changed"));
    }
    let work = crate::message_resolution::owner::execution(
        tx,
        &target.session_id,
        &delegation.target_message_id(),
    )
    .await?;
    let MessageExecution::Owned(owner) = work else {
        return Err(invalid(
            "WorkHub resume requires an exclusively owned Message",
        ));
    };
    match &event.fact {
        Fact::WorkhubResumeObserved { target, .. } => {
            if owner.invocation != *target || matches!(owner.state, InvocationState::Ended { .. }) {
                return Err(invalid("Observed WorkHub execution is no longer running"));
            }
            candidates::require_available(tx, &target.session_id, Some(target)).await?;
        }
        Fact::InvocationOpened {
            input: InvocationInput::Continuation { claim, .. },
            ..
        } => {
            if owner.invocation != claim.source.invocation
                || !matches!(
                    owner.state,
                    InvocationState::Ended {
                        outcome: InvocationOutcome::Failed { .. }
                            | InvocationOutcome::Cancelled { .. },
                        ..
                    }
                )
            {
                return Err(invalid(
                    "WorkHub continuation no longer owns its failed/cancelled source",
                ));
            }
            // Continuation validation already authenticates the exact raw prefix,
            // workspace and high-water before this domain guard.
            candidates::require_available(tx, &target.session_id, None).await?;
        }
        _ => unreachable!(),
    }
    Ok(())
}
