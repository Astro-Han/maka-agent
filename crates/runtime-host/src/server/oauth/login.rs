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
use maka_client_capability::{Registration, broker::ServiceCall};
use maka_config::{
    ConfigError,
    oauth::enrollment::{LoginCompletion, PreparedLogin},
};
use maka_model::oauth::{Client, ErrorKind};
use serde_json::json;
use std::{sync::atomic::Ordering, time::Duration};

pub(super) async fn run(
    host: &Host,
    attempt: &Attempt,
    ticket: PreparedLogin,
    registration: Arc<Registration>,
) -> Phase {
    let result = authorize(host, attempt, &ticket, registration).await;
    settle(host, attempt, ticket, result).await
}

async fn settle(
    host: &Host,
    attempt: &Attempt,
    ticket: PreparedLogin,
    result: Result<maka_model::oauth::Tokens, Failure>,
) -> Phase {
    match result {
        Ok(tokens) => {
            // A successful admitted grant wins cancellation, including root
            // drain. Commit is not a cancellable continuation of the RPC.
            {
                let mut progress = attempt.progress.lock().unwrap_or_else(|e| e.into_inner());
                progress.deferred = true;
                progress.phase = Phase::Committing;
            }
            let result = match (
                serde_json::to_string(&tokens),
                super::super::configuration::now(),
            ) {
                (Ok(secret), Ok(now)) => {
                    let _admission = host.executions.lock_admission().await;
                    ticket.complete(secret, now).await
                }
                _ => {
                    return Phase::Failed {
                        failure: Failure::InternalFailure,
                    };
                }
            };
            match result {
                Ok(LoginCompletion::Committed(_)) => {
                    let revision = host.change_revision.fetch_add(1, Ordering::SeqCst) + 1;
                    let _ = host
                        .changes
                        .send(json!({"kind":"configuration.changed","revision":revision}));
                    Phase::Authenticated
                }
                Ok(LoginCompletion::Superseded { connection, .. }) => Phase::Failed {
                    failure: if connection {
                        Failure::ConnectionChanged
                    } else {
                        Failure::CredentialChanged
                    },
                },
                Ok(LoginCompletion::AttemptConflict) => Phase::Failed {
                    failure: Failure::CredentialChanged,
                },
                Ok(LoginCompletion::SlugTaken) => Phase::Failed {
                    failure: Failure::SlugTaken,
                },
                Err(error) => {
                    if matches!(error, ConfigError::CommitUnknown) {
                        host.draining.cancel();
                    }
                    Phase::Failed {
                        failure: Failure::PersistenceFailed,
                    }
                }
            }
        }
        Err(failure) => {
            let progress = attempt.progress.lock().unwrap_or_else(|e| e.into_inner());
            if !progress.deferred && attempt.cancellation.is_cancelled() {
                Phase::Cancelled
            } else {
                Phase::Failed { failure }
            }
        }
    }
}
async fn authorize(
    host: &Host,
    attempt: &Attempt,
    ticket: &PreparedLogin,
    registration: Arc<Registration>,
) -> Result<maka_model::oauth::Tokens, Failure> {
    let network = ticket.network_configuration();
    let policy = maka_network::Policy::from_settings(&network.proxy, network.password.as_deref())
        .map_err(|_| Failure::AuthorizationFailed)?;
    let client = Client::new(&policy).map_err(|_| Failure::InternalFailure)?;
    let authorization = client
        .start(attempt.connection.provider_type, &attempt.cancellation)
        .await
        .map_err(provider_failure)?;
    let input =
        json!({"url":authorization.verification_url(), "stateHint":authorization.user_code()});
    oauth::decode_presentation("open_external", &input).map_err(|_| Failure::InternalFailure)?;
    let call = host
        .capabilities
        .broker
        .prepare_service(
            registration,
            ServiceCall {
                service_id: oauth::PRESENTATION_SERVICE_ID.into(),
                version: oauth::PRESENTATION_SERVICE_VERSION.into(),
                method: "open_external".into(),
                input: input.as_object().expect("presentation object").clone(),
            },
            Duration::from_secs(150),
            attempt.cancellation.clone(),
        )
        .map_err(|_| Failure::CapabilityUnavailable)?;
    let result = call
        .accepted()
        .await
        .map_err(|_| Failure::CapabilityUnavailable)?
        .admit()
        .await
        .map_err(|_| Failure::CapabilityUnavailable)?;
    if !result.content.is_empty()
        || !result
            .structured_content
            .as_ref()
            .is_some_and(Value::is_object)
    {
        return Err(Failure::CapabilityUnavailable);
    }
    oauth::decode_presentation_result("open_external", result.structured_content.as_ref().unwrap())
        .map_err(|_| Failure::AuthorizationFailed)?;
    {
        let mut progress = attempt.progress.lock().unwrap_or_else(|e| e.into_inner());
        if attempt.cancellation.is_cancelled() {
            return Err(Failure::AuthorizationFailed);
        }
        progress.phase = Phase::Exchanging;
    }
    authorization
        .finish(&attempt.cancellation, |boundary| attempt.boundary(boundary))
        .await
        .map_err(provider_failure)
}
fn provider_failure(error: maka_model::oauth::Error) -> Failure {
    match error.kind {
        ErrorKind::InvalidGrant | ErrorKind::InvalidToken | ErrorKind::EntitlementDenied => {
            Failure::ProviderRejected
        }
        _ => Failure::AuthorizationFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_config::oauth::enrollment::LoginPreparation;
    use maka_event_log::root::{RootNamespaces, RootOwner};
    use maka_model::oauth::{PollBoundary, Tokens};

    #[tokio::test]
    async fn cancel_racing_admission_never_publishes_terminal_before_spent_grant_commit() {
        let temp = tempfile::tempdir().unwrap();
        let namespaces = RootNamespaces {
            ownership: temp.path().join("owners"),
            control: temp.path().join("control"),
        };
        let host = Host::open(RootOwner::create(&temp.path().join("root"), &namespaces).unwrap())
            .await
            .unwrap();
        let input = LoginStart {
            attempt_id: "cancel-race".into(),
            target: oauth::Target::Create {
                provider_type: Provider::XaiOauth,
                slug: None,
                name: None,
            },
        };
        let LoginPreparation::Ready(ticket) = host
            .configuration
            .prepare_oauth_login(input.clone())
            .await
            .unwrap()
        else {
            panic!("expected unpublished ticket");
        };
        let attempt = Attempt {
            input,
            connection: ticket.identity().clone(),
            progress: Mutex::new(Progress {
                phase: Phase::Exchanging,
                deferred: false,
            }),
            cancellation: host.draining.child_token(),
        };
        // Deterministically place cancellation between the transport's check
        // and its admission callback, an actual multi-thread interleaving.
        attempt.cancel();
        assert_eq!(attempt.projection().phase, Phase::Exchanging);
        attempt.boundary(PollBoundary::Admitted);
        host.draining.cancel();
        let mut changes = host.changes.subscribe();
        let phase = settle(
            &host,
            &attempt,
            *ticket,
            Ok(Tokens {
                access_token: "synthetic-spent-grant".into(),
                refresh_token: "synthetic-refresh".into(),
                expires_at: 9_000_000_000_000,
                id_token: None,
                token_type: Some("Bearer".into()),
                scope: None,
                base_url: None,
                account_id: None,
                account_uuid: None,
            }),
        )
        .await;
        assert_eq!(phase, Phase::Authenticated);
        let receipt = host
            .configuration
            .oauth_login_receipt("cancel-race".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(receipt.connection, attempt.connection);
        assert_eq!(
            host.configuration
                .catalog()
                .await
                .unwrap()
                .connections
                .len(),
            1
        );
        assert_eq!(
            changes.try_recv().unwrap(),
            json!({"kind":"configuration.changed","revision":1})
        );
        host.log.shutdown().await.unwrap();
        host.configuration.shutdown().await.unwrap();
    }
}
