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

use maka_model::oauth::{Client, ErrorKind, PollBoundary};
use maka_runtime::oauth::Provider;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
mod fixture;
use fixture::{authorization, oracle, proxy};

#[tokio::test]
async fn refresh_matches_ts_and_preserves_provider_specific_authority_over_pinned_proxy() {
    use maka_model::oauth::Tokens;
    tokio::time::timeout(Duration::from_secs(20), async {
        for provider in [Provider::OpenaiCodex, Provider::XaiOauth, Provider::GithubCopilot] {
            for rotated in [None, Some(""), Some("rotated +/&")] {
                let previous = json!({"access_token":"gho_previous", "refresh_token":"old +/&",
                    "expires_at":0,"id_token":"old-id","account_id":"account","account_uuid":"uuid",
                    "token_type":"Bearer","scope":"old-scope","base_url":"https://api.githubcopilot.com"});
                let mut payload = json!({"access_token":"ghu_next", "expires_in":3600});
                if let Some(token) = rotated { payload["refresh_token"] = token.into(); }
                if provider == Provider::XaiOauth { payload.as_object_mut().unwrap().remove("expires_in"); }
                let responses = vec![json!({"status":200,"payload":payload})];
                let expected = fixture::refresh_oracle(provider, &responses, &previous).await;
                let (client, server) = proxy(responses).await;
                let tokens = client.refresh(provider, Tokens::from_stored(&previous.to_string()).unwrap()).await.unwrap();
                let mut value = serde_json::to_value(tokens).unwrap();
                assert!(value["expires_at"].as_u64().unwrap() > 1_000_000);
                value.as_object_mut().unwrap().remove("expires_at");
                assert_eq!(value, expected["tokens"]);
                assert_eq!(server.await.unwrap(), expected["requests"].as_array().unwrap().clone());
            }
        }
        let previous = json!({"access_token":"gho_evergreen","refresh_token":"gho_evergreen",
            "expires_at":9_007_199_254_740_991u64});
        let (client, server) = proxy(vec![]).await;
        assert_eq!(serde_json::to_value(client.refresh(Provider::GithubCopilot,
            Tokens::from_stored(&previous.to_string()).unwrap()).await.unwrap()).unwrap(), previous);
        assert!(server.await.unwrap().is_empty());
    }).await.unwrap();
}

#[tokio::test]
async fn refresh_rejects_invalid_credentials_and_unusable_replacements_without_retry() {
    use maka_model::oauth::Tokens;
    let previous = json!({"access_token":"gho_old","refresh_token":"refresh","expires_at":0});
    for malformed in [
        json!([]),
        json!({"expires_at":0}),
        json!({"access_token":"a","refresh_token":"r","expires_at":-1}),
        json!({"access_token":"a","refresh_token":"r","expires_at":0,"account_id":{}}),
    ] {
        assert!(Tokens::from_stored(&malformed.to_string()).is_err());
    }
    assert!(matches!(Tokens::from_stored(&" ".repeat(64 * 1024 + 1)),
        Err(error) if error.kind == ErrorKind::ResponseTooLarge));
    tokio::time::timeout(Duration::from_secs(20), async {
        for payload in [
            json!({"error":"invalid_grant"}),
            json!({"access_token":"not-a-github-token","expires_in":3600}),
            json!({"access_token":"gho_next","expires_in":0}),
            json!({"access_token":"gho_next","expires_in":3600,"refresh_token":null}),
        ] {
            let (client, server) = proxy(vec![json!({"status":200,"payload":payload})]).await;
            assert!(
                client
                    .refresh(
                        Provider::GithubCopilot,
                        Tokens::from_stored(&previous.to_string()).unwrap()
                    )
                    .await
                    .is_err()
            );
            assert_eq!(server.await.unwrap().len(), 1);
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn device_grants_match_ts_over_authenticated_proxy_and_survive_admitted_cancellation() {
    tokio::time::timeout(Duration::from_secs(20), async {
        for provider in [Provider::OpenaiCodex, Provider::XaiOauth, Provider::GithubCopilot] {
            let mut responses = vec![authorization(provider)];
            if provider == Provider::OpenaiCodex {
                responses.push(json!({"status":200,"payload":{"authorization_code":"approved +/&","code_verifier":"verifier +/&"}}));
            }
            responses.push(json!({"status":200,"payload":{
                "access_token":if provider == Provider::GithubCopilot { "gho_synthetic" } else { "synthetic" },
                "refresh_token":"refresh +/&","expires_in":3600,"token_type":"Bearer","scope":"model"}}));
            if provider == Provider::GithubCopilot {
                responses.push(json!({"status":200,"payload":{"data":[usable_model()]}}));
            }
            let expected = oracle(provider, &responses).await;
            let (client, server) = proxy(responses).await;
            let cancel = CancellationToken::new();
            let device = client.start(provider, &cancel).await.unwrap();
            assert_eq!(device.user_code(), "M4KA");
            assert!(device.verification_url().starts_with("https://"));
            let mut boundaries = Vec::new();
            let tokens = device.finish(&cancel, |boundary| {
                boundaries.push(boundary);
                if boundary == PollBoundary::Admitted { cancel.cancel(); }
            }).await.unwrap();
            assert_eq!(boundaries[0], PollBoundary::Admitted);
            assert_eq!(boundaries.len(), if provider == Provider::OpenaiCodex { 2 } else { 1 });
            let mut tokens = serde_json::to_value(tokens).unwrap();
            assert!(tokens["expires_at"].as_u64().unwrap() > 1_000_000);
            tokens.as_object_mut().unwrap().remove("expires_at");
            assert_eq!(tokens, expected["tokens"]);
            assert_eq!(server.await.unwrap(), expected["requests"].as_array().unwrap().clone());
        }
    }).await.unwrap();
}

fn usable_model() -> Value {
    json!({"id":"copilot-model","model_picker_enabled":true,
        "supported_endpoints":["/responses"],"capabilities":{"supports":{"tool_calls":true}}})
}

#[tokio::test]
async fn copilot_entitlement_distinguishes_account_refusal_from_inconclusive_failures() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut blocked = usable_model();
        blocked["policy"] = json!({"state":"unconfigured"});
        let mut malformed = usable_model();
        malformed["supported_endpoints"] = json!("/responses");
        for (response, denied) in [
            (json!({"status":403,"payload":{}}), true),
            (json!({"status":429,"payload":{}}), false),
            (json!({"status":200,"payload":{"data":[blocked]}}), true),
            (
                json!({"status":200,"payload":{"data":[usable_model(),malformed]}}),
                false,
            ),
        ] {
            let provider = Provider::GithubCopilot;
            let responses = vec![
                authorization(provider),
                json!({"status":200,"payload":{"access_token":"gho_synthetic"}}),
                response,
            ];
            let expected = oracle(provider, &responses).await;
            assert_eq!(
                expected["error"],
                if denied {
                    "GitHubCopilotEntitlementError"
                } else {
                    "GitHubCopilotEntitlementUnavailableError"
                }
            );
            let (client, server) = proxy(responses).await;
            let cancel = CancellationToken::new();
            let device = client.start(provider, &cancel).await.unwrap();
            let error = device.finish(&cancel, |_| {}).await.err().unwrap();
            assert_eq!(
                error.kind,
                if denied {
                    ErrorKind::EntitlementDenied
                } else {
                    ErrorKind::EntitlementUnavailable
                }
            );
            assert_eq!(
                server.await.unwrap(),
                expected["requests"].as_array().unwrap().clone()
            );
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn pending_restores_cancellation_and_malformed_redirected_or_denied_grants_fail_closed() {
    tokio::time::timeout(Duration::from_secs(20), async {
        for provider in [Provider::OpenaiCodex, Provider::XaiOauth, Provider::GithubCopilot] {
            let status = match provider { Provider::OpenaiCodex => 403, Provider::XaiOauth => 400, Provider::GithubCopilot => 200 };
            let (client, server) = proxy(vec![authorization(provider), json!({"status":status,"payload":{"error":"authorization_pending"}})]).await;
            let cancel = CancellationToken::new();
            let device = client.start(provider, &cancel).await.unwrap();
            let mut boundaries = Vec::new();
            let failure = device.finish(&cancel, |boundary| {
                boundaries.push(boundary);
                if boundary == PollBoundary::Admitted { cancel.cancel(); }
            }).await.err().unwrap();
            assert_eq!(failure.kind, ErrorKind::Aborted);
            assert_eq!(boundaries, vec![PollBoundary::Admitted, PollBoundary::Retry]);
            assert_eq!(server.await.unwrap().len(), 2);
            if provider != Provider::OpenaiCodex {
                let (client, server) = proxy(vec![authorization(provider), json!({
                    "status":status,"payload":{"error":"access_denied"}
                })]).await;
                let cancel = CancellationToken::new();
                let device = client.start(provider, &cancel).await.unwrap();
                assert_eq!(device.finish(&cancel, |_| {}).await.err().unwrap().kind, ErrorKind::InvalidGrant);
                assert_eq!(server.await.unwrap().len(), 2);
            }
        }
        for response in [
            json!({"status":307,"payload":{},"headers":"Location: https://auth.openai.com/oauth/token\r\n"}),
            json!({"status":200,"raw":"x".repeat(65537)}),
            json!({"status":200,"raw":"{invalid json"}),
        ] {
            let (client, server) = proxy(vec![response.clone()]).await;
            let error = client.start(Provider::OpenaiCodex, &CancellationToken::new()).await.err().unwrap();
            assert_eq!(error.kind, if response["status"] == 307 { ErrorKind::ProviderRejected }
                else if response["raw"].as_str().unwrap().len() > 65536 { ErrorKind::ResponseTooLarge }
                else { ErrorKind::InvalidResponse });
            assert_eq!(server.await.unwrap().len(), 1);
        }
        let mut bad_url = authorization(Provider::XaiOauth);
        bad_url["payload"]["verification_uri_complete"] = json!("https://x.ai.evil.invalid/device");
        let (client, server) = proxy(vec![bad_url]).await;
        assert_eq!(client.start(Provider::XaiOauth, &CancellationToken::new()).await.err().unwrap().kind, ErrorKind::InvalidResponse);
        server.await.unwrap();
        let mut expired = authorization(Provider::XaiOauth);
        expired["payload"]["expires_in"] = json!(1);
        let (client, server) = proxy(vec![expired]).await;
        let cancel = CancellationToken::new();
        let device = client.start(Provider::XaiOauth, &cancel).await.unwrap();
        assert_eq!(device.finish(&cancel, |_| panic!("no grant may be dispatched after expiry")).await.err().unwrap().kind, ErrorKind::Expired);
        assert_eq!(server.await.unwrap().len(), 1);
    }).await.unwrap();
}
