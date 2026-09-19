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

use super::{Host, HostError};
use crate::session::SessionConfiguration;
use maka_event_log::sessions::SessionRecord;
use maka_protocol::{Operation, OperationError, OperationErrorCode as Code, Outcome, workhub};
use maka_runtime::workhub::COORDINATION_SESSION_ID;
use serde_json::Value;

mod action;
mod candidates;
pub(super) use action::ERRORS as ACTION_ERRORS;
pub(super) use candidates::ERRORS as CANDIDATE_ERRORS;

pub(super) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::OperationConflict,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
];

fn control(
    host: &Host,
) -> Result<
    maka_plugins::contributions::Contribution<crate::plugins::workhub::Control>,
    OperationError,
> {
    host.executions
        .plugin_catalog
        .snapshot::<crate::plugins::workhub::Control>(&maka_plugins::composition::Scope::Profile)
        .entries
        .remove(crate::plugins::workhub::ID)
        .ok_or_else(|| failure(Code::OperationUnavailable, "WorkHub is unavailable"))
}

pub(super) const TURN_ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::NotFound,
    Code::SessionArchived,
    Code::SessionBusy,
    Code::OperationConflict,
    Code::PersistenceFailed,
    Code::CommitOutcomeUnknown,
    Code::InternalFailure,
];

pub(super) async fn execute(
    host: &std::sync::Arc<Host>,
    operation: Operation,
    value: &Value,
    connection_id: uuid::Uuid,
) -> Result<Outcome, HostError> {
    let result = match operation {
        Operation::WorkhubCoordinationResolve => match control(host) {
            Ok(control) => control.value.resolve().await.and_then(serialize),
            Err(error) => Err(error),
        },
        Operation::WorkhubCoordinationQuery => match control(host) {
            Ok(control) => control.value.query().await.and_then(serialize),
            Err(error) => Err(error),
        },
        Operation::WorkhubCoordinationActFromTurn => {
            let input = workhub::decode_act(value)?;
            if let Err(error) = admit_callback(
                host,
                connection_id,
                &input.turn_id,
                input.action_id.as_str(),
            )
            .await
            {
                return Ok(Outcome::failure(error));
            }
            action::act(host, input, connection_id)
                .await
                .and_then(serialize)
        }
        Operation::WorkhubCoordinationSelectAndDelegate => {
            let input = workhub::decode_selection(value)?;
            if let Err(error) = admit_callback(
                host,
                connection_id,
                &input.turn_id,
                input.action_id.as_str(),
            )
            .await
            {
                return Ok(Outcome::failure(error));
            }
            match control(host) {
                Ok(control) => control.value.select(input).await.and_then(serialize),
                Err(error) => Err(error),
            }
        }
        Operation::WorkhubCoordinationCandidates => candidates::query(host)
            .await
            .and_then(|candidates| serialize(candidates.result)),
        Operation::WorkhubCoordinationAnswer => {
            let input = workhub::decode_answer_input(value)?;
            answer(host, input, connection_id).await.and_then(serialize)
        }
        Operation::WorkhubCoordinationConfigureModel => {
            let input = workhub::decode_model_input(value)?;
            match control(host) {
                Ok(control) => control
                    .value
                    .configure_model(input)
                    .await
                    .and_then(serialize),
                Err(error) => Err(error),
            }
        }
        _ => unreachable!("validated WorkHub control operation"),
    };
    match result {
        Ok(value) => {
            workhub::decode_output(operation, &value)?;
            if !matches!(
                operation,
                Operation::WorkhubCoordinationQuery | Operation::WorkhubCoordinationCandidates
            ) {
                host.session_catalog
                    .publish_session(&host.changes, COORDINATION_SESSION_ID)
                    .await?;
            }
            Ok(Outcome::success(value))
        }
        Err(error) => Ok(Outcome::failure(error)),
    }
}

async fn answer(
    host: &std::sync::Arc<Host>,
    input: workhub::AnswerInput,
    connection_id: uuid::Uuid,
) -> Result<workhub::TurnResult, OperationError> {
    let request = crate::plugins::workhub::answer::Request::new(input)?;
    if let Some(receipt) = host.executions.workhub_answer_receipt(&request).await? {
        return Ok(receipt);
    }
    control(host)?.value.answer(request, connection_id).await
}

fn serialize(value: impl serde::Serialize) -> Result<Value, OperationError> {
    serde_json::to_value(value).map_err(|e| failure(Code::InternalFailure, e.to_string()))
}

pub(super) async fn record(
    host: &Host,
) -> Result<Option<SessionRecord<SessionConfiguration>>, OperationError> {
    host.executions.workhub_coordinator().await
}

/// Ordinary RPC residency pins this check through the eventual reply. During
/// preparation only the exact already-admitted client tool may create work.
async fn admit_callback(
    host: &Host,
    connection: uuid::Uuid,
    turn: &str,
    action: &str,
) -> Result<(), OperationError> {
    let _admission = host.executions.lock_admission().await;
    let phase = *host.retirement.lock().unwrap_or_else(|e| e.into_inner());
    match phase {
        super::retirement::Phase::Ready if !host.draining.is_cancelled() => Ok(()),
        super::retirement::Phase::Preparing
            if host.capabilities.broker.admitted_tool(
                connection,
                COORDINATION_SESSION_ID,
                turn,
                action,
                "desktop_workhub",
                "tasks",
            ) =>
        {
            Ok(())
        }
        _ => Err(failure(
            Code::HostDraining,
            "Host is not admitting new WorkHub work",
        )),
    }
}

fn failure(code: Code, message: impl Into<String>) -> OperationError {
    OperationError {
        code,
        message: message.into().chars().take(1024).collect(),
    }
}
