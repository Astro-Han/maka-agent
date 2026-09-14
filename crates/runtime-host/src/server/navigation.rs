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
use maka_protocol::{
    Operation, OperationError, OperationErrorCode as Code, Outcome, navigation as wire,
};
use serde_json::Value;

pub(super) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::NotFound,
    Code::PersistenceFailed,
    Code::InternalFailure,
];

pub(super) async fn execute(
    host: &Host,
    operation: Operation,
    value: &Value,
) -> Result<Outcome, HostError> {
    let output = match operation {
        Operation::SessionTurnsQuery => {
            let input = wire::decode_turns_input(value)?;
            let current = match host.log.navigation_fence(&input.session_id).await {
                Ok(fence) => fence,
                Err(error) => return Ok(Outcome::failure(stored(error))),
            };
            if input
                .through_sequence
                .is_some_and(|through| current.is_none_or(|now| through > now))
            {
                return Ok(failure(
                    Code::InvalidRequest,
                    "Turn navigation fence is beyond settled history",
                ));
            }
            let through = input.through_sequence.or(current);
            let (contributions, next_position) = if let Some(through) = through {
                if let Err(error) = prepare(host, &input.session_id, through).await {
                    return Ok(Outcome::failure(error));
                }
                match host
                    .log
                    .navigation_turns(
                        &input.session_id,
                        through,
                        input.position,
                        input.max_contributions,
                    )
                    .await
                {
                    Ok(page) => (page.contributions, page.next_position),
                    Err(error) => return Ok(Outcome::failure(stored(error))),
                }
            } else {
                (Vec::new(), None)
            };
            serde_json::to_value(wire::TurnsResult {
                session_id: input.session_id,
                through_sequence: through,
                contributions,
                next_position,
            })?
        }
        Operation::SessionTurnLandmarksQuery => {
            let input = wire::decode_landmarks_input(value)?;
            let through = match host.log.navigation_fence(&input.session_id).await {
                Ok(fence) => fence,
                Err(error) => return Ok(Outcome::failure(stored(error))),
            };
            let landmarks = if let Some(through) = through {
                if let Err(error) = prepare(host, &input.session_id, through).await {
                    return Ok(Outcome::failure(error));
                }
                match host
                    .log
                    .navigation_landmarks(&input.session_id, through, input.max_landmarks)
                    .await
                {
                    Ok(rows) => rows,
                    Err(error) => return Ok(Outcome::failure(stored(error))),
                }
            } else {
                Vec::new()
            };
            serde_json::to_value(wire::LandmarksResult {
                session_id: input.session_id,
                through_sequence: through,
                landmarks,
            })?
        }
        _ => return Err("not a navigation operation".into()),
    };
    wire::decode_output(operation, &output)?;
    Ok(Outcome::success(output))
}

async fn prepare(host: &Host, session: &str, through: u64) -> Result<(), OperationError> {
    loop {
        if host.draining.is_cancelled() {
            return Err(OperationError {
                code: Code::HostDraining,
                message: "Host is draining".into(),
            });
        }
        if host
            .log
            .prepare_transcript(session, through / 256, 32)
            .await
            .map_err(stored)?
        {
            return Ok(());
        }
        // Each transaction is bounded and commits only disposable index work.
        tokio::task::yield_now().await;
    }
}
fn stored(error: StoreError) -> OperationError {
    let code = match &error {
        StoreError::SessionNotFound => Code::NotFound,
        StoreError::PrefixTooLarge | StoreError::Projection(_) => Code::OperationUnavailable,
        _ => Code::PersistenceFailed,
    };
    OperationError {
        code,
        message: error.to_string(),
    }
}
fn failure(code: Code, message: &str) -> Outcome {
    Outcome::failure(OperationError {
        code,
        message: message.into(),
    })
}
