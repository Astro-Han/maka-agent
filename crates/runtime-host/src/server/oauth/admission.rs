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
use maka_plugins::provider::{Binding, Connection, Context, authentication::Authenticate};
use std::panic::AssertUnwindSafe;

pub(super) async fn start(
    host: &Arc<Host>,
    connection: uuid::Uuid,
    input: LoginStart,
) -> Result<LoginProjection, OperationError> {
    let _gate = host.oauth.admission.lock().await;
    let fingerprint = input
        .fingerprint()
        .map_err(|_| failure(Code::InvalidRequest, "Invalid authentication input"))?;
    if let Some((prior, projection)) = host
        .oauth
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .find(&input.attempt_id)
    {
        return if prior == fingerprint {
            Ok(projection)
        } else {
            Err(conflict())
        };
    }
    // Historical idempotency precedes plugin availability and interactive capacity.
    if let Some(receipt) = host
        .configuration
        .oauth_login_receipt(input.attempt_id.clone())
        .await
        .map_err(|_| failure(Code::PersistenceFailed, "Login receipt query failed"))?
    {
        if !receipt.matches(&input) {
            return Err(conflict());
        }
        let projection = LoginProjection {
            attempt_id: input.attempt_id,
            connection: receipt.connection,
            phase: receipt_phase(receipt.phase),
        };
        host.oauth
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remember(fingerprint, projection.clone());
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
            "Another login is in progress",
        ));
    }
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let mut prepared = match host
        .configuration
        .prepare_oauth_login(input.clone())
        .await
        .map_err(|_| failure(Code::PersistenceFailed, "Login admission failed"))?
    {
        LoginPreparation::Ready(ticket) => ticket,
        LoginPreparation::Finished(receipt) | LoginPreparation::OutcomeUnknown(receipt) => {
            let projection = LoginProjection {
                attempt_id: input.attempt_id,
                connection: receipt.connection,
                phase: receipt_phase(receipt.phase),
            };
            host.oauth
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remember(fingerprint, projection.clone());
            return Ok(projection);
        }
        LoginPreparation::Rejected(reason) => {
            return Err(match reason {
                LoginRejection::AttemptConflict => conflict(),
                LoginRejection::SlugTaken => {
                    failure(Code::SlugTaken, "Connection slug is already taken")
                }
                LoginRejection::ConnectionNotFound => {
                    failure(Code::NotFound, "Connection was not found")
                }
                LoginRejection::CatalogFull => {
                    failure(Code::OperationConflict, "Connection capacity is exhausted")
                }
                LoginRejection::ConnectionChanged => {
                    failure(Code::OperationConflict, "Connection changed")
                }
                LoginRejection::AttemptsFull => failure(
                    Code::OperationConflict,
                    "Unresolved login capacity is exhausted",
                ),
            });
        }
    };
    let _admission = host.executions.lock_admission().await;
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let provider = Binding::resolve(
        &prepared.identity().provider,
        &host.executions.plugin_catalog,
    )
    .map_err(|_| {
        failure(
            Code::OperationUnavailable,
            "Connection provider is unavailable",
        )
    })?;
    if matches!(&input.target, oauth::Target::Create { .. }) {
        let configuration = provider
            .definition()
            .configure(prepared.connection().configuration.clone())
            .map_err(|_| failure(Code::InvalidRequest, "Invalid provider configuration"))?;
        prepared
            .configure_creation(configuration)
            .map_err(|_| failure(Code::InvalidRequest, "Invalid provider configuration"))?;
    }
    let method = provider
        .definition()
        .descriptor()
        .authentication
        .iter()
        .find(|method| method.id == input.authentication.method)
        .ok_or_else(|| failure(Code::InvalidRequest, "Unknown authentication method"))?;
    let registration = if method.interactive {
        Some(
            host.capabilities
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
                        "Initiating Client cannot present this login",
                    )
                })?,
        )
    } else {
        None
    };
    let attempt = Arc::new(Attempt {
        input,
        connection: prepared.identity().clone(),
        phase: Mutex::new(if method.interactive {
            Phase::AwaitingAuthorization
        } else {
            Phase::Exchanging
        }),
        cancellation: host.draining.child_token(),
    });
    let network = prepared.network_configuration();
    let policy =
        maka_network::Policy::from_host_settings(&network.proxy, network.password.as_deref())
            .map_err(|_| failure(Code::OperationUnavailable, "Invalid network configuration"))?;
    let context = Context {
        transport: host.executions.model_transport(&policy).map_err(|_| {
            failure(
                Code::OperationUnavailable,
                "Provider transport is unavailable",
            )
        })?,
        cancellation: attempt.cancellation.clone(),
        interaction: registration
            .map(|registration| login::interaction(host.clone(), attempt.clone(), registration)),
    };
    let call = provider
        .prepare_authenticate(
            Authenticate {
                connection: Connection {
                    id: prepared.connection().connection_id.clone(),
                    revision: prepared.connection().revision,
                    configuration: prepared.connection().configuration.clone(),
                },
                method: attempt.input.authentication.method.clone(),
                input: attempt.input.authentication.input.clone(),
            },
            context,
        )
        .map_err(|_| {
            failure(
                Code::InvalidRequest,
                "Provider authentication was not admitted",
            )
        })?;
    if !prepared
        .claim()
        .await
        .map_err(super::super::configuration::failure)?
    {
        return query(host, attempt.input.attempt_id.clone()).await;
    }
    let projection = attempt.projection();
    host.oauth
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .active = Some(attempt.clone());
    let owner = host.clone();
    host.requests.spawn(async move {
        let phase = match AssertUnwindSafe(login::run(&owner, &attempt, *prepared, call))
            .catch_unwind()
            .await
        {
            Ok(phase) => phase,
            Err(_) => Phase::Failed {
                failure: Failure::OutcomeUnknown,
            },
        };
        *attempt.phase.lock().unwrap_or_else(|e| e.into_inner()) = phase;
        let mut state = owner.oauth.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active = None;
        state.remember(fingerprint, attempt.projection());
    });
    Ok(projection)
}

fn conflict() -> OperationError {
    failure(
        Code::InvalidRequest,
        "Login attemptId is already bound to another request",
    )
}
