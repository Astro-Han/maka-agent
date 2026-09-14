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
use maka_event_log::StoreError;
use maka_presentation::shell::RESOURCE_REF_PREFIX;
use maka_protocol::{
    OperationError, OperationErrorCode as Code, Outcome,
    resource::{ResourceQueryInput as Input, ResourceQueryResult as Output},
};
pub(in crate::server) mod controller;
mod mutation;

pub(super) fn supports(operation: maka_protocol::Operation) -> bool {
    use maka_protocol::Operation::*;
    maka_protocol::resource::is_controller(operation)
        || matches!(
            operation,
            RuntimeResourceQuery | RuntimeResourceStart | RuntimeResourceStop
        )
}

pub(super) async fn execute(
    host: &Host,
    connection: uuid::Uuid,
    operation: maka_protocol::Operation,
    input: &serde_json::Value,
) -> Result<Outcome, HostError> {
    use maka_protocol::{Operation::*, resource};
    match operation {
        RuntimeResourceControllerAcquire => Ok(controller::acquire(
            host,
            connection,
            resource::decode_controller_identity(input)?,
        )
        .await),
        RuntimeResourceControllerControl => Ok(controller::control(
            host,
            connection,
            resource::decode_controller_control(input)?,
        )
        .await),
        RuntimeResourceControllerRelease => Ok(controller::release(
            host,
            connection,
            resource::decode_controller_identity(input)?,
        )
        .await),
        RuntimeResourceQuery => query(host, resource::decode_query_input(input)?).await,
        RuntimeResourceStart => mutation::start(host, resource::decode_start_input(input)?).await,
        RuntimeResourceStop => mutation::stop(host, resource::decode_stop_input(input)?).await,
        _ => unreachable!("validated resource operation"),
    }
}

pub(super) async fn query(host: &Host, input: Input) -> Result<Outcome, HostError> {
    let session = input.session_id().to_owned();
    let id = if let Input::Get { resource_ref, .. } = &input {
        let Some(id) = resource_ref
            .strip_prefix(RESOURCE_REF_PREFIX)
            .filter(|id| maka_runtime::interaction::entity_id(id).is_ok())
        else {
            return Ok(failure(
                Code::InvalidRequest,
                "Runtime Resource ref is unsupported",
            ));
        };
        Some(id)
    } else {
        None
    };
    let offset = match &input {
        Input::ListContinue { cursor, .. } => cursor
            .parse::<u64>()
            .ok()
            .filter(|n| *n <= 9_007_199_254_740_991 && n.to_string() == *cursor),
        _ => Some(0),
    };
    let page = match host
        .log
        .query_shell_resources(&session, id, offset.unwrap_or(0))
        .await
    {
        Ok(page) => page,
        Err(StoreError::SessionNotFound) => {
            return Ok(failure(Code::NotFound, "Session was not found"));
        }
        Err(error) => {
            host.draining.cancel();
            eprintln!("Runtime Resource canonical read failed: {error}");
            return Ok(failure(
                Code::InternalFailure,
                "Runtime Resource state is unavailable",
            ));
        }
    };
    let result = match input {
        Input::Get { .. } => Output::Resource {
            session_id: session,
            revision: page.revision,
            resource: page.resources.into_iter().next().map(Box::new),
        },
        Input::ListContinue { revision, .. } if revision != page.revision => {
            Output::RevisionChanged {
                expected: revision,
                actual: page.revision,
            }
        }
        Input::ListContinue { .. } if offset.is_none_or(|n| n == 0 || n >= page.total) => {
            return Ok(failure(
                Code::InvalidRequest,
                "Runtime Resource cursor is invalid",
            ));
        }
        _ => Output::Page {
            session_id: session,
            revision: page.revision,
            resources: page.resources,
            next_cursor: page.next_offset.map(|n| n.to_string()),
        },
    };
    Ok(Outcome::success(serde_json::to_value(result)?))
}
fn failure(code: Code, message: &str) -> Outcome {
    Outcome::failure(OperationError {
        code,
        message: message.into(),
    })
}
