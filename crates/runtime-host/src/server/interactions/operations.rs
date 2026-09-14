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

use super::{Interactions, failure};
use crate::server::{Host, HostError};
use maka_protocol::{
    Operation, OperationError, OperationErrorCode as Code, Outcome, ProtocolError,
    interaction::{self, InteractionAnswerInput, InteractionSnapshot},
};
use serde_json::Value;

pub(crate) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::InteractionQuery | Operation::InteractionAnswer
    )
}
pub(crate) fn errors(operation: Operation) -> Option<&'static [Code]> {
    const QUERY: &[Code] = &[
        Code::HostNotReady,
        Code::HostDraining,
        Code::OperationUnavailable,
        Code::InvalidRequest,
        Code::NotFound,
        Code::InternalFailure,
    ];
    const ANSWER: &[Code] = &[
        Code::HostNotReady,
        Code::HostDraining,
        Code::OperationUnavailable,
        Code::InvalidRequest,
        Code::NotFound,
        Code::OperationConflict,
        Code::AlreadyResolved,
        Code::InternalFailure,
    ];
    match operation {
        Operation::InteractionQuery => Some(QUERY),
        Operation::InteractionAnswer => Some(ANSWER),
        _ => None,
    }
}
pub(crate) fn decode_input(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    let result = match operation {
        Operation::InteractionQuery => {
            serde_json::to_value(interaction::decode_query_input(value)?)
        }
        Operation::InteractionAnswer => {
            serde_json::to_value(interaction::decode_answer_input(value)?)
        }
        _ => return Err(ProtocolError::invalid("Unknown Interaction operation")),
    };
    result.map_err(|error| ProtocolError::invalid(error.to_string()))
}
pub(crate) fn decode_output(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    let snapshot = match operation {
        Operation::InteractionQuery => interaction::decode_snapshot(value)?,
        Operation::InteractionAnswer => interaction::decode_answered_snapshot(value)?,
        _ => return Err(ProtocolError::invalid("Unknown Interaction operation")),
    };
    serde_json::to_value(snapshot).map_err(|error| ProtocolError::invalid(error.to_string()))
}
pub(crate) async fn execute(
    host: &Host,
    operation: Operation,
    input: &Value,
) -> Result<Outcome, HostError> {
    let result = match operation {
        Operation::InteractionQuery => {
            let input = interaction::decode_query_input(input)?;
            let _gate = host.interactions.admission.lock().await;
            host.interactions
                .query_record(&input.session_id, &input.interaction_id)
                .await
                .and_then(|record| InteractionSnapshot::from_record(&record).map_err(internal))
        }
        Operation::InteractionAnswer => {
            host.interactions
                .answer(interaction::decode_answer_input(input)?)
                .await
        }
        _ => unreachable!("validated Interaction operation"),
    };
    Ok(match result {
        Ok(snapshot) => {
            Outcome::success(decode_output(operation, &serde_json::to_value(snapshot)?)?)
        }
        Err(error) => Outcome::failure(error),
    })
}
impl Interactions {
    async fn answer(
        &self,
        input: InteractionAnswerInput,
    ) -> Result<InteractionSnapshot, OperationError> {
        let _gate = self.admission.lock().await;
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let record = self
            .query_record(&input.session_id, &input.interaction_id)
            .await?;
        let record = if record.outcome.is_none() {
            input
                .answer
                .validate_for_request(&record.request)
                .map_err(|message| failure(Code::OperationConflict, message))?;
            self.commit_outcome(
                &record.request_id,
                input
                    .answer
                    .clone()
                    .into_outcome(crate::server::configuration::now().map_err(internal)?),
            )
            .await?
            .record
        } else {
            record
        };
        if !record
            .outcome
            .as_ref()
            .is_some_and(|outcome| input.answer.matches_outcome(outcome))
        {
            return Err(failure(
                Code::AlreadyResolved,
                "Interaction is already resolved",
            ));
        }
        InteractionSnapshot::from_record(&record).map_err(internal)
    }
}
fn internal(error: impl std::fmt::Display) -> OperationError {
    failure(Code::InternalFailure, &error.to_string())
}
