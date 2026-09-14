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
use maka_config::oauth::enrollment::LoginReceipt;
use maka_protocol::{Operation, OperationError, OperationErrorCode as Code, Outcome, oauth};
use maka_runtime::oauth::{Failure, LoginProjection, LoginStart, Phase, Provider};
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
#[derive(Default)]
struct State {
    active: Option<Arc<Attempt>>,
    terminal: VecDeque<(LoginStart, LoginProjection)>,
}
struct Attempt {
    input: LoginStart,
    connection: oauth::ConnectionIdentity,
    progress: Mutex<Progress>,
    cancellation: CancellationToken,
}
struct Progress {
    phase: Phase,
    deferred: bool,
}
impl Attempt {
    fn projection(&self) -> LoginProjection {
        LoginProjection {
            attempt_id: self.input.attempt_id.clone(),
            connection: self.connection.clone(),
            phase: self
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .phase,
        }
    }
    fn cancel(&self) {
        // Cancellation is a request, not a terminal fact. Only the owner can
        // decide whether a concurrent poll already spent the grant.
        self.cancellation.cancel();
    }
    fn boundary(&self, boundary: maka_model::oauth::PollBoundary) {
        use maka_model::oauth::PollBoundary;
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        progress.deferred = boundary != PollBoundary::Retry;
    }
}
impl State {
    fn find(&self, id: &str) -> Option<(LoginStart, LoginProjection)> {
        if let Some(active) = &self.active
            && active.input.attempt_id == id
        {
            return Some((active.input.clone(), active.projection()));
        }
        self.terminal
            .iter()
            .find(|(input, _)| input.attempt_id == id)
            .cloned()
    }
    fn remember(&mut self, input: LoginStart, projection: LoginProjection) {
        self.terminal
            .retain(|(prior, _)| prior.attempt_id != input.attempt_id);
        self.terminal.push_back((input, projection));
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
            Ok(serde_json::to_value(oauth::EnrollmentProjection {
                provider,
                enabled: enabled(provider),
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
    let (input, projection) = authenticated(id, receipt);
    host.oauth
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remember(input, projection.clone());
    Ok(projection)
}
fn authenticated(id: String, receipt: LoginReceipt) -> (LoginStart, LoginProjection) {
    (
        LoginStart {
            attempt_id: id.clone(),
            target: receipt.target,
        },
        LoginProjection {
            attempt_id: id,
            connection: receipt.connection,
            phase: Phase::Authenticated,
        },
    )
}
fn enabled(provider: Provider) -> bool {
    match provider {
        Provider::XaiOauth => true,
        Provider::OpenaiCodex => {
            std::env::var("MAKA_CODEX_SUBSCRIPTION_EXPERIMENTAL").as_deref() != Ok("0")
        }
        Provider::GithubCopilot => {
            std::env::var("MAKA_GITHUB_COPILOT_DEVICE_LOGIN_EXPERIMENTAL").as_deref() == Ok("1")
        }
    }
}
fn failure(code: Code, message: &'static str) -> OperationError {
    OperationError {
        code,
        message: message.into(),
    }
}
