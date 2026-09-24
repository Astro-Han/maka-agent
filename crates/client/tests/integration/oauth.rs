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

use super::connection::pair_with;
use maka_client::{ClientError, RequestFailure};
use maka_protocol::{Operation, oauth::*};
use serde_json::json;
use std::time::Duration;

fn provider(name: &str) -> Identity {
    Identity {
        package_id: "example.providers".into(),
        entry_id: "example.providers".into(),
        scope: maka_protocol::model_provider::Scope::Profile,
        name: name.into(),
    }
}

fn start() -> LoginStart {
    LoginStart {
        attempt_id: "login-attempt".into(),
        target: Target::Create {
            provider: provider("subscription"),
            configuration: json!({}),
            slug: "personal-codex".into(),
            name: "Personal".into(),
        },
        authentication: maka_protocol::oauth::AuthenticationInput {
            method: "login".into(),
            input: json!({"key":"transient-secret"}),
        },
    }
}

fn projection() -> LoginProjection {
    LoginProjection {
        attempt_id: start().attempt_id,
        connection: ConnectionIdentity {
            connection_id: "b746eb13-287c-4f3a-8590-dac93c0a1253".into(),
            slug: "personal-codex".into(),
            provider: provider("subscription"),
        },
        phase: Phase::AwaitingAuthorization,
    }
}

#[tokio::test]
async fn enrollment_binds_provider_without_guessing_availability() {
    for (actual, enabled, valid) in [
        (provider("subscription"), true, true),
        (provider("subscription"), false, true),
        (provider("other"), true, false),
    ] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let task = tokio::spawn({
            let client = client.clone();
            async move { client.oauth_enrollment(provider("subscription")).await }
        });
        let request = reader.read().await.unwrap().unwrap();
        assert_eq!(request["operation"], "oauth.enrollment.query");
        assert_eq!(
            request["input"],
            json!({"provider":provider("subscription")})
        );
        writer
            .write(
                &json!({"requestId":request["requestId"],"operation":request["operation"],
            "ok":true,"result":{"provider":actual,"enabled":enabled}}),
            )
            .await
            .unwrap();
        if valid {
            assert_eq!(
                task.await.unwrap().unwrap(),
                EnrollmentProjection {
                    provider: actual,
                    enabled
                }
            );
        } else {
            assert!(matches!(
                task.await.unwrap(),
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
            tokio::time::timeout(Duration::from_secs(1), client.closed())
                .await
                .unwrap();
        }
        client.disconnect();
    }
}

#[tokio::test]
async fn login_lifecycle_preserves_host_phases_and_freezes_attempt_and_connection() {
    // Each mutation changes one otherwise valid projection. These are binding
    // errors, not decoder errors; accepting them could authorize another account.
    for operation in [
        Operation::OauthLoginStart,
        Operation::OauthLoginQuery,
        Operation::OauthLoginCancel,
    ] {
        for case in 0..10 {
            let input = start();
            let known = projection().connection;
            let mut result = projection();
            match case {
                0 => {}
                1 => result.phase = Phase::Exchanging,
                2 => result.phase = Phase::Committing,
                3 => result.phase = Phase::Authenticated,
                4 => result.phase = Phase::Cancelled,
                5 => {
                    result.phase = Phase::Failed {
                        failure: Failure::ProviderRejected,
                    }
                }
                6 => result.attempt_id = "other-attempt".into(),
                7 => result.connection.provider = provider("other"),
                8 => result.connection.slug = "other-slug".into(),
                9 => result.connection.connection_id = "connection-two".into(),
                _ => unreachable!(),
            }
            let valid = case < 6 || (case == 9 && operation == Operation::OauthLoginStart);
            let (client, mut notices, mut reader, mut writer) =
                pair_with(maka_client::Operations).await;
            let task = tokio::spawn({
                let client = client.clone();
                let input = input.clone();
                async move {
                    match operation {
                        Operation::OauthLoginStart => client.start_oauth_login(&input).await,
                        Operation::OauthLoginQuery => {
                            client
                                .query_oauth_login(&input.recovery(), Some(&known))
                                .await
                        }
                        Operation::OauthLoginCancel => {
                            client
                                .cancel_oauth_login(&input.recovery(), Some(&known))
                                .await
                        }
                        _ => unreachable!(),
                    }
                }
            });
            let request = reader.read().await.unwrap().unwrap();
            assert_eq!(request["operation"], json!(operation));
            assert_eq!(
                request["input"],
                if operation == Operation::OauthLoginStart {
                    json!(input)
                } else {
                    json!({"attemptId":input.attempt_id})
                }
            );
            writer
                .write(&json!({"kind":"connection.catalog.changed","revision":7}))
                .await
                .unwrap();
            writer
                .write(
                    &json!({"requestId":request["requestId"],"operation":request["operation"],
                "ok":true,"result":result}),
                )
                .await
                .unwrap();
            assert!(matches!(
                notices.recv().await.unwrap(),
                maka_client::Notification::Catalog(_)
            ));
            if valid {
                assert_eq!(task.await.unwrap().unwrap(), result);
                // This barrier proves query/cancel did not queue another start,
                // poll, cancellation, or inferred "completion" on our behalf.
                let barrier = tokio::spawn({
                    let client = client.clone();
                    async move { client.oauth_enrollment(provider("subscription")).await }
                });
                let next = reader.read().await.unwrap().unwrap();
                assert_eq!(next["operation"], "oauth.enrollment.query");
                writer.write(&json!({"requestId":next["requestId"],"operation":next["operation"],"ok":true,
                    "result":{"provider":provider("subscription"),"enabled":true}})).await.unwrap();
                barrier.await.unwrap().unwrap();
            } else {
                assert!(matches!(
                    task.await.unwrap(),
                    Err(RequestFailure::Unknown(ClientError::Protocol(_)))
                ));
                tokio::time::timeout(Duration::from_secs(1), client.closed())
                    .await
                    .unwrap();
            }
            client.disconnect();
        }
    }
}

#[tokio::test]
async fn recovery_uses_original_target_and_rejection_does_not_become_success() {
    for existing in [false, true] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let mut input = start();
        if existing {
            input.target = Target::Existing {
                expected: maka_protocol::configuration::ConnectionCredentialTarget {
                    connection_id: projection().connection.connection_id,
                    revision: 1,
                    slug: "personal-codex".into(),
                    provider: provider("subscription"),
                    configuration: json!({}),
                },
                configuration: json!({}),
            };
        }
        let saved = serde_json::to_value(input.recovery()).unwrap();
        assert!(saved.get("authentication").is_none());
        let recovered: LoginRecovery = serde_json::from_value(saved).unwrap();
        recovered.validate().unwrap();
        let query = tokio::spawn({
            let client = client.clone();
            async move { client.query_oauth_login(&recovered, None).await }
        });
        let request = reader.read().await.unwrap().unwrap();
        assert_eq!(request["operation"], "oauth.login.query");
        assert_eq!(request["input"], json!({"attemptId":input.attempt_id}));
        writer
            .write(
                &json!({"requestId":request["requestId"],"operation":request["operation"],
            "ok":false,"error":{"code":"not_found","message":"No attempt"}}),
            )
            .await
            .unwrap();
        assert!(matches!(
            query.await.unwrap(),
            Err(RequestFailure::Rejected(ClientError::Rejected(_)))
        ));

        let query = tokio::spawn({
            let client = client.clone();
            async move { client.query_oauth_login(&input.recovery(), None).await }
        });
        let request = reader.read().await.unwrap().unwrap();
        let mut wrong = projection();
        if existing {
            wrong.connection.connection_id = "other-connection".into();
        } else {
            wrong.connection.provider = provider("other");
        }
        writer
            .write(
                &json!({"requestId":request["requestId"],"operation":request["operation"],
            "ok":true,"result":wrong}),
            )
            .await
            .unwrap();
        assert!(matches!(
            query.await.unwrap(),
            Err(RequestFailure::Unknown(ClientError::Protocol(_)))
        ));
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn invalid_local_basis_dispatches_nothing_and_does_not_close_connection() {
    let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
    let mut input = start();
    input.attempt_id = "invalid attempt".into();
    assert!(matches!(
        client.start_oauth_login(&input).await,
        Err(RequestFailure::NotDispatched(_))
    ));
    assert!(matches!(
        client.query_oauth_login(&input.recovery(), None).await,
        Err(RequestFailure::NotDispatched(_))
    ));
    let mut wrong = projection().connection;
    wrong.slug = "wrong-slug".into();
    assert!(matches!(
        client
            .cancel_oauth_login(&start().recovery(), Some(&wrong))
            .await,
        Err(RequestFailure::NotDispatched(_))
    ));
    let barrier = tokio::spawn({
        let client = client.clone();
        async move { client.oauth_enrollment(provider("subscription")).await }
    });
    let request = reader.read().await.unwrap().unwrap();
    assert_eq!(request["operation"], "oauth.enrollment.query");
    writer
        .write(
            &json!({"requestId":request["requestId"],"operation":request["operation"],"ok":true,
        "result":{"provider":provider("subscription"),"enabled":false}}),
        )
        .await
        .unwrap();
    assert!(!barrier.await.unwrap().unwrap().enabled);
    client.disconnect();
}
