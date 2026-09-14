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

use super::*;
use futures_util::FutureExt;
use maka_config::oauth::enrollment::{LoginPreparation, LoginRejection};
use std::panic::AssertUnwindSafe;

pub(super) async fn start(
    host: &Arc<Host>,
    connection: uuid::Uuid,
    input: LoginStart,
) -> Result<LoginProjection, OperationError> {
    let _gate = host.oauth.admission.lock().await;
    if let Some((prior, projection)) = host
        .oauth
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .find(&input.attempt_id)
    {
        return if prior == input {
            Ok(projection)
        } else {
            Err(conflict())
        };
    }
    // Receipts are historical idempotency, not proof that current credentials
    // still exist. Replay precedes provider gates and interactive capacity.
    if let Some(receipt) = host
        .configuration
        .oauth_login_receipt(input.attempt_id.clone())
        .await
        .map_err(|_| failure(Code::PersistenceFailed, "OAuth receipt query failed"))?
    {
        if receipt.target != input.target {
            return Err(conflict());
        }
        let (input, projection) = authenticated(input.attempt_id, receipt);
        host.oauth
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remember(input, projection.clone());
        return Ok(projection);
    }
    if host
        .oauth
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .active
        .is_some()
    {
        return Err(failure(
            Code::OperationConflict,
            "Another OAuth login is in progress",
        ));
    }
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let prepared = match host
        .configuration
        .prepare_oauth_login(input.clone())
        .await
        .map_err(|_| failure(Code::PersistenceFailed, "OAuth login admission failed"))?
    {
        LoginPreparation::Ready(ticket) => ticket,
        LoginPreparation::Authenticated(receipt) => {
            let (input, projection) = authenticated(input.attempt_id, receipt);
            host.oauth
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remember(input, projection.clone());
            return Ok(projection);
        }
        LoginPreparation::Rejected(reason) => {
            return Err(match reason {
                LoginRejection::AttemptConflict => conflict(),
                LoginRejection::SlugTaken => {
                    failure(Code::SlugTaken, "Connection slug is already taken")
                }
                LoginRejection::ConnectionNotFound => {
                    failure(Code::NotFound, "OAuth connection was not found")
                }
                LoginRejection::CatalogFull => {
                    failure(Code::OperationConflict, "Connection capacity is exhausted")
                }
                LoginRejection::ProviderUnavailable => failure(
                    Code::InvalidRequest,
                    "Connection cannot start an OAuth login",
                ),
            });
        }
    };
    // Share the existing execution admission cut with capability retirement.
    let _admission = host.executions.lock_admission().await;
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    if !enabled(prepared.identity().provider_type) {
        return Err(failure(
            Code::OperationUnavailable,
            "OAuth enrollment is disabled",
        ));
    }
    let registration = host
        .capabilities
        .registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .current_for_connection(connection)
        .filter(|registration| {
            registration.available()
                && registration
                    .manifest()
                    .services
                    .as_ref()
                    .is_some_and(|services| {
                        services.iter().any(|service| {
                            service.service_id == oauth::PRESENTATION_SERVICE_ID
                                && service.version == oauth::PRESENTATION_SERVICE_VERSION
                        })
                    })
        })
        .ok_or_else(|| {
            failure(
                Code::CapabilityUnavailable,
                "Initiating Client cannot present this OAuth login",
            )
        })?;
    let attempt = Arc::new(Attempt {
        input,
        connection: prepared.identity().clone(),
        progress: Mutex::new(Progress {
            phase: Phase::AwaitingAuthorization,
            deferred: false,
        }),
        cancellation: host.draining.child_token(),
    });
    let projection = attempt.projection();
    host.oauth
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .active = Some(attempt.clone());
    let owner = host.clone();
    // Register before the admitting request can release its residency. The
    // existing Host drain waits this owner before closing stores/root authority.
    host.requests.spawn(async move {
        let phase = match AssertUnwindSafe(login::run(&owner, &attempt, *prepared, registration))
            .catch_unwind()
            .await
        {
            Ok(phase) => phase,
            Err(_) => {
                owner.draining.cancel();
                Phase::Failed {
                    failure: Failure::InternalFailure,
                }
            }
        };
        attempt
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .phase = phase;
        let mut state = owner.oauth.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active = None;
        state.remember(attempt.input.clone(), attempt.projection());
    });
    Ok(projection)
}
fn conflict() -> OperationError {
    failure(
        Code::InvalidRequest,
        "OAuth attemptId is already bound to another target",
    )
}
