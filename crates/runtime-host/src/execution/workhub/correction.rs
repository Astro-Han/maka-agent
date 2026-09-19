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
    super::{Executions, Result, failure},
    commands::{WorkHubCommands, stored},
};
use crate::plugins::workhub::{
    control::Identity,
    correction::{Request, validate_receipt},
    target::Target,
};
use maka_event_log::workhub::correction::CorrectionRecord;
use maka_plugins::fiber::Context;
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::{
    event::Invocation,
    input::InvocationInput,
    workhub::{ActionId, CorrectionRequest, Delegation, DelegationDelivery},
};
use std::sync::Arc;
use uuid::Uuid;

mod settlement;
pub(super) use settlement::{recover, settle};

pub(super) async fn probe(
    executions: &Arc<Executions>,
    caller: Context,
    action_id: ActionId,
    turn_id: String,
) -> Result<Option<CorrectionRecord>> {
    let _gate = executions.lock_admission().await;
    let _call = caller
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    let receipt = executions
        .log
        .workhub_correction(&action_id)
        .await
        .map_err(|error| stored(executions, error))?;
    if receipt.is_none() {
        validate_new(executions, &action_id, &turn_id).await?;
    }
    Ok(receipt)
}

async fn validate_new(
    executions: &Executions,
    action_id: &ActionId,
    turn_id: &str,
) -> Result<maka_event_log::turns::TurnBoundary> {
    if executions
        .log
        .workhub_action(action_id)
        .await
        .map_err(|error| stored(executions, error))?
        .is_some()
        || executions
            .log
            .workhub_stop(action_id)
            .await
            .map_err(|error| stored(executions, error))?
            .is_some()
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub action belongs to another operation",
        ));
    }
    if executions.shutdown.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let source = executions.workhub_source(turn_id).await?;
    if !matches!(source.root_input(), InvocationInput::Message { .. }) {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub correction requires a user message",
        ));
    }
    Ok(source)
}

pub(super) async fn execute(
    commands: &WorkHubCommands,
    executions: &Arc<Executions>,
    caller: Context,
    request: Request,
) -> Result<CorrectionRecord> {
    let _call = caller
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    {
        let _gate = executions.lock_admission().await;
        if let Some(record) = executions
            .log
            .workhub_correction(&request.identity.action_id)
            .await
            .map_err(|error| stored(executions, error))?
        {
            validate_receipt(&request.identity, &record)?;
        } else {
            // Repeat source and instance admission after lock-free preparation.
            let _admission = caller
                .admit()
                .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
            let source = validate_new(
                executions,
                &request.identity.action_id,
                &request.identity.turn_id,
            )
            .await?;
            let InvocationInput::Message { content, .. } = source.root_input() else {
                unreachable!("validated message boundary")
            };
            commands
                .validate_target(executions, &request.target)
                .await?;
            let intent = CorrectionRequest {
                action_id: request.identity.action_id.clone(),
                request_fingerprint: request.identity.fingerprint.clone(),
                source: source.invocation.clone(),
                source_message_event_id: source.root_opening_event_id().to_owned(),
                replaces_action_id: request.replaces_action_id,
                target: request.target.correction(),
                delegation_text: request.text.unwrap_or_else(|| content.text.clone()),
            };
            delegation(executions, &request.target, &intent)
                .message(content)
                .map_err(|reason| failure(Code::OperationConflict, reason))?;
            executions
                .log
                .request_workhub_correction(
                    intent,
                    matches!(&request.target, Target::Existing { .. })
                        .then(|| request.target.revision()),
                    executions.active_session_owner(request.target.id()),
                )
                .await
                .map_err(|error| stored(executions, error))?;
        }
    }
    settle(commands, executions, request.identity).await
}

fn delegation(executions: &Executions, target: &Target, request: &CorrectionRequest) -> Delegation {
    let owner = executions.active_session_owner(target.id());
    let delivery = match (&owner, target) {
        (
            Some(_),
            Target::Existing {
                configuration_digest,
                ..
            },
        ) => DelegationDelivery::Steering {
            configuration_digest: configuration_digest.clone(),
        },
        _ => DelegationDelivery::NewTurn,
    };
    Delegation {
        action_id: request.action_id.clone(),
        kind: target.kind(),
        description: Some(target.description()),
        delivery,
        request_fingerprint: request.request_fingerprint.clone(),
        source_message_event_id: request.source_message_event_id.clone(),
        target: owner.unwrap_or_else(|| Invocation {
            session_id: target.id().into(),
            turn_id: Uuid::new_v4().to_string(),
            run_id: Uuid::new_v4().to_string(),
            invocation_id: Uuid::new_v4().to_string(),
        }),
        target_revision: target.revision(),
        delegation_text: request.delegation_text.clone(),
    }
}
