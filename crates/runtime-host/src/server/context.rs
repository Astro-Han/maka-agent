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

use super::{Host, HostError, turns};
use maka_event_log::context::LatestMainContext;
use maka_protocol::{
    Operation, OperationError, OperationErrorCode as Code, Outcome, ProtocolError, Result,
    context as wire,
};
use serde_json::Value;
use std::time::UNIX_EPOCH;

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ContextCompact | Operation::ContextDiagnosticsQuery
    )
}

pub(super) fn decode_input(operation: Operation, value: &Value) -> Result<Value> {
    let input = match operation {
        Operation::ContextCompact => {
            serde_json::to_value(wire::decode_context_compact_input(value)?)
        }
        Operation::ContextDiagnosticsQuery => {
            serde_json::to_value(wire::decode_context_diagnostics_input(value)?)
        }
        _ => return Err(ProtocolError::invalid("Unknown Context operation")),
    };
    input.map_err(|error| ProtocolError::invalid(error.to_string()))
}

pub(super) fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::ContextCompact => {
            wire::decode_context_compact_result(value)?;
        }
        Operation::ContextDiagnosticsQuery => {
            wire::decode_context_diagnostics_result(value)?;
        }
        _ => return Err(ProtocolError::invalid("Unknown Context operation")),
    }
    Ok(value.clone())
}

pub(super) fn errors(operation: Operation) -> Option<&'static [Code]> {
    match operation {
        Operation::ContextCompact => turns::errors(Operation::TurnStart),
        Operation::ContextDiagnosticsQuery => Some(&[
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::NotFound,
            Code::InternalFailure,
        ]),
        _ => None,
    }
}

pub(super) async fn execute(
    host: &Host,
    operation: Operation,
    input: &Value,
) -> std::result::Result<Outcome, HostError> {
    let result = match operation {
        Operation::ContextCompact => {
            let input = wire::decode_context_compact_input(input)?;
            let result = host.executions.compact(input.clone()).await;
            if let Ok(output) = &result {
                wire::assert_compact_output_for_input(&input, output)?;
            }
            result.map(serde_json::to_value)
        }
        Operation::ContextDiagnosticsQuery => {
            let input = wire::decode_context_diagnostics_input(input)?;
            diagnostics(host, &input.session_id)
                .await
                .map(serde_json::to_value)
        }
        _ => unreachable!("validated Context operation"),
    };
    match result {
        Ok(value) => Ok(Outcome::success(decode_output(operation, &value?)?)),
        Err(error) => Ok(Outcome::failure(error)),
    }
}

async fn diagnostics(
    host: &Host,
    session: &str,
) -> std::result::Result<wire::ContextDiagnosticsResult, OperationError> {
    use wire::{ContextDiagnosticsResult as Output, ContextDiagnosticsUnavailableReason as Reason};
    let unavailable = |reason| Output::Unavailable { reason };
    if host
        .log
        .get_session::<Value>(session)
        .await
        .map_err(internal)?
        .is_none()
    {
        return Err(OperationError {
            code: Code::NotFound,
            message: "Session does not exist".into(),
        });
    }
    let selected = match host
        .log
        .latest_main_context(session)
        .await
        .map_err(internal)?
    {
        LatestMainContext::NoCompletedRequest => {
            return Ok(unavailable(Reason::NoCompletedRequest));
        }
        LatestMainContext::TraceUnavailable => return Ok(unavailable(Reason::TraceUnavailable)),
        LatestMainContext::Selected(selected) => selected,
    };
    let Some(context) = selected.context else {
        return Ok(unavailable(Reason::TraceUnavailable));
    };
    let Some(completed_at) = selected
        .recorded_at
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .filter(|millis| *millis <= 9_007_199_254_740_991)
    else {
        return Ok(unavailable(Reason::TraceUnavailable));
    };
    Ok(Output::Available {
        provider_id: context.provider_id,
        model_id: selected.model_id,
        completed_at,
        input_tokens: selected.usage.input_tokens,
        cache_read_input_tokens: selected.usage.cache_read_tokens,
        context_window: context.context_window,
        composition: None,
        compaction: None,
    })
}

fn internal(error: impl std::fmt::Display) -> OperationError {
    OperationError {
        code: Code::InternalFailure,
        message: error.to_string(),
    }
}
