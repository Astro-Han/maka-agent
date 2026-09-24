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

mod admission;
mod login;

use super::{Host, HostError};
use maka_protocol::{Operation, OperationError, OperationErrorCode as Code, Outcome, oauth};
use maka_runtime::oauth::{Failure, LoginProjection, LoginStart, Phase};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

/// One interactive authorization per root. Provider transport and credential
/// storage remain separate; neither owns the client's presentation lifetime.
#[derive(Default)]
pub(super) struct Coordinator {
    admission: tokio::sync::Mutex<()>,
    state: Mutex<State>,
}

impl Coordinator {
    pub(super) fn active_count(&self) -> usize {
        usize::from(
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .active
                .is_some(),
        )
    }
}
#[derive(Default)]
struct State {
    active: Option<Arc<Attempt>>,
    terminal: VecDeque<(String, LoginProjection)>,
}
struct Attempt {
    input: LoginStart,
    connection: oauth::ConnectionIdentity,
    phase: Mutex<Phase>,
    cancellation: CancellationToken,
}
impl Attempt {
    fn projection(&self) -> LoginProjection {
        LoginProjection {
            attempt_id: self.input.attempt_id.clone(),
            connection: self.connection.clone(),
            phase: *self.phase.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
    fn cancel(&self) {
        // Cancellation is a request, not a terminal fact. Only the owner can
        // decide whether a concurrent poll already spent the grant.
        self.cancellation.cancel();
    }
}
impl State {
    fn find(&self, id: &str) -> Option<(String, LoginProjection)> {
        if let Some(active) = &self.active
            && active.input.attempt_id == id
        {
            return Some((
                active.input.fingerprint().expect("validated login input"),
                active.projection(),
            ));
        }
        self.terminal
            .iter()
            .find(|(_, projection)| projection.attempt_id == id)
            .cloned()
    }
    fn remember(&mut self, fingerprint: String, projection: LoginProjection) {
        self.terminal
            .retain(|(_, prior)| prior.attempt_id != projection.attempt_id);
        self.terminal.push_back((fingerprint, projection));
        while self.terminal.len() > 256 {
            self.terminal.pop_front();
        }
    }
}

pub(super) async fn execute(
    host: &Arc<Host>,
    connection: uuid::Uuid,
    operation: Operation,
    input: &Value,
) -> Result<Outcome, HostError> {
    let result = match operation {
        Operation::OauthEnrollmentQuery => {
            let provider = oauth::decode_enrollment(input)?.provider;
            let enabled = maka_plugins::provider::Binding::resolve(
                &provider,
                &host.executions.plugin_catalog,
            )
            .is_ok_and(|binding| !binding.definition().descriptor().authentication.is_empty());
            Ok(serde_json::to_value(oauth::EnrollmentProjection {
                provider,
                enabled,
            })?)
        }
        Operation::OauthLoginStart => {
            let input = oauth::decode_start(input)?;
            admission::start(host, connection, input.clone())
                .await
                .and_then(|output| {
                    oauth::assert_start(&input, &output)
                        .map_err(|_| failure(Code::InternalFailure, "OAuth identity changed"))?;
                    Ok(serde_json::to_value(output).expect("OAuth projection is serializable"))
                })
        }
        Operation::OauthLoginQuery | Operation::OauthLoginCancel => {
            let input = oauth::decode_attempt(input)?;
            if operation == Operation::OauthLoginCancel {
                let state = host.oauth.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(attempt) = &state.active
                    && attempt.input.attempt_id == input.attempt_id
                {
                    attempt.cancel();
                }
            }
            query(host, input.attempt_id).await.map(|output| {
                serde_json::to_value(output).expect("OAuth projection is serializable")
            })
        }
        _ => unreachable!("validated OAuth operation"),
    };
    Ok(match result {
        Ok(value) => Outcome::success(oauth::decode_output(operation, &value)?),
        Err(error) => Outcome::failure(error),
    })
}
async fn query(host: &Host, id: String) -> Result<LoginProjection, OperationError> {
    if let Some((_, projection)) = host
        .oauth
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .find(&id)
    {
        return Ok(projection);
    }
    let receipt = host
        .configuration
        .oauth_login_receipt(id.clone())
        .await
        .map_err(|_| failure(Code::PersistenceFailed, "OAuth receipt query failed"))?
        .ok_or_else(|| failure(Code::NotFound, "OAuth login was not found"))?;
    Ok(LoginProjection {
        attempt_id: id,
        connection: receipt.connection,
        phase: receipt_phase(receipt.phase),
    })
}
fn receipt_phase(phase: Phase) -> Phase {
    if phase == Phase::Exchanging {
        Phase::Failed {
            failure: Failure::OutcomeUnknown,
        }
    } else {
        phase
    }
}
fn failure(code: Code, message: &'static str) -> OperationError {
    OperationError {
        code,
        message: message.into(),
    }
}
