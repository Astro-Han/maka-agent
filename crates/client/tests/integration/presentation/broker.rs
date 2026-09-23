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

use super::pair_with;
use maka_client_capability::{
    Endpoint, Identity, PrincipalKind, Registry,
    broker::{Broker, CallError, ServiceCall},
};
use maka_protocol::{capability, oauth};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
async fn host_broker_accepts_presentation_failure_cancel_and_releases_real_registration() {
    let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
    let broker = Arc::new(Broker::default());
    let mut registry = Registry::default();
    let connection = Uuid::new_v4();
    let (endpoint, mut frames) = Endpoint::channel(8);
    let provider = registry
        .attach(
            connection,
            Identity {
                principal_kind: PrincipalKind::LocalOwner,
                principal_id: "local-owner".into(),
                client_instance_id: "rust-tui".into(),
                credential_bound_client_instance_id: None,
                capability_owner: None,
            },
            endpoint,
        )
        .unwrap();
    let publishing = tokio::spawn({
        let client = client.clone();
        async move { client.publish_oauth_presentation().await }
    });
    let request = reader.read().await.unwrap().unwrap();
    let manifest = capability::decode_replace_input(&request["input"]).unwrap();
    let result = registry.replace(connection, manifest).unwrap();
    writer.write(&json!({"requestId":request["requestId"],"operation":request["operation"],"ok":true,"result":result})).await.unwrap();
    let mut service = publishing.await.unwrap().unwrap();
    let registration = registry.current(&provider).unwrap();
    let stop = CancellationToken::new();
    let forwarding = tokio::spawn({
        let broker = broker.clone();
        let stop = stop.clone();
        async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    frame = frames.recv() => {
                        let Some(frame) = frame else { break; };
                        writer.write(&frame).await.unwrap();
                    }
                    frame = reader.read() => {
                        let Some(frame) = frame.unwrap() else { break; };
                        broker.accept(connection, capability::decode_client_frame(&frame).unwrap()).unwrap();
                    }
                }
            }
        }
    });
    for outcome in ["presented", "failed", "cancelled"] {
        let cancel = CancellationToken::new();
        let call = broker
            .prepare_service(
                registration.clone(),
                ServiceCall {
                    service_id: oauth::PRESENTATION_SERVICE_ID.into(),
                    version: oauth::PRESENTATION_SERVICE_VERSION.into(),
                    method: "open_external".into(),
                    input: json!({"url":"https://login.example/device","stateHint":"TEST-1234"})
                        .as_object()
                        .unwrap()
                        .clone(),
                },
                Duration::from_secs(5),
                cancel.clone(),
            )
            .unwrap();
        let accepted = tokio::time::timeout(Duration::from_secs(2), call.accepted())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(accepted.evidence()).unwrap(),
            json!({"kind":"none"})
        );
        let completing = tokio::spawn(accepted.admit());
        let presentation = tokio::time::timeout(Duration::from_secs(2), service.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(presentation.state_hint.as_deref(), Some("TEST-1234"));
        match outcome {
            "presented" => assert!(presentation.acknowledge_presented()),
            "failed" => drop(presentation),
            "cancelled" => {
                cancel.cancel();
                tokio::time::timeout(Duration::from_secs(2), presentation.cancelled())
                    .await
                    .unwrap();
                assert!(!presentation.acknowledge_presented());
            }
            _ => unreachable!(),
        }
        let result = tokio::time::timeout(Duration::from_secs(2), completing)
            .await
            .unwrap()
            .unwrap();
        match outcome {
            "presented" => {
                let result = result.unwrap();
                assert!(result.content.is_empty());
                assert_eq!(
                    oauth::decode_presentation_result(
                        "open_external",
                        result.structured_content.as_ref().unwrap()
                    )
                    .unwrap(),
                    oauth::PresentationResult::Presented
                );
            }
            "failed" => assert!(matches!(result, Err(CallError::ProviderFailed(_)))),
            "cancelled" => assert!(matches!(result, Err(CallError::OutcomeUnknown(_)))),
            _ => unreachable!(),
        }
    }
    registry
        .unregister(connection, &service.registration_id)
        .unwrap();
    drop(registration);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), service.recv())
            .await
            .unwrap()
            .is_none()
    );
    broker.shutdown().await;
    stop.cancel();
    forwarding.await.unwrap();
    client.disconnect();
    // This fixture uses the actual Host broker, not a provider login/token
    // endpoint. No account, token, or durable connection is created.
}
