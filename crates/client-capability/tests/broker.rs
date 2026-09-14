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

use base64::{Engine, engine::general_purpose::STANDARD};
use maka_client_capability::{
    Endpoint, Identity, PrincipalKind, Registry,
    broker::{Broker, CallError, PendingCall, ServiceCall},
};
use maka_runtime::capability::{AdmissionEvidence, ClientFrame, HostFrame, Manifest, ServiceOffer};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn setup(
    transport: CancellationToken,
) -> (
    Registry,
    Uuid,
    Arc<maka_client_capability::Registration>,
    tokio::sync::mpsc::Receiver<HostFrame>,
) {
    let mut registry = Registry::default();
    let connection = Uuid::new_v4();
    let (endpoint, outbound) = Endpoint::channel_with_cancellation(64, transport);
    let provider = registry
        .attach(
            connection,
            Identity {
                principal_kind: PrincipalKind::LocalOwner,
                principal_id: "owner".into(),
                client_instance_id: "client".into(),
                credential_bound_client_instance_id: None,
                capability_owner: None,
            },
            endpoint,
        )
        .unwrap();
    registry
        .replace(
            connection,
            Manifest {
                registration_id: "r".into(),
                offers: vec![],
                services: Some(vec![ServiceOffer {
                    service_id: "service".into(),
                    version: "1".into(),
                }]),
            },
        )
        .unwrap();
    let registration = registry.current(&provider).unwrap();
    (registry, connection, registration, outbound)
}
fn prepare(
    broker: &Broker,
    registration: &Arc<maka_client_capability::Registration>,
) -> Result<PendingCall, CallError> {
    broker.prepare_service(
        registration.clone(),
        ServiceCall {
            service_id: "service".into(),
            version: "1".into(),
            method: "perform".into(),
            input: serde_json::Map::new(),
        },
        Duration::from_secs(5),
        CancellationToken::new(),
    )
}
fn provider_accept(id: &str) -> ClientFrame {
    ClientFrame::Accepted {
        invocation_id: id.into(),
        admission_evidence: AdmissionEvidence::None,
    }
}
fn is_control(frame: HostFrame, kind: &str, id: &str) {
    assert_eq!(
        serde_json::to_value(frame).unwrap(),
        json!({"kind":format!("client.capability.{kind}"),"invocationId":id})
    );
}

#[tokio::test(start_paused = true)]
async fn acceptance_waits_for_host_admission_then_assembles_exact_chunks_and_releases_pins() {
    let (mut registry, connection, registration, mut out) = setup(CancellationToken::new());
    let broker = Broker::default();
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    assert!(matches!(
        out.recv().await.unwrap(),
        HostFrame::ServiceCall { .. }
    ));
    broker.accept(connection, provider_accept(&id)).unwrap();
    let accepted = pending.accepted().await.unwrap();
    assert_eq!(accepted.evidence(), &AdmissionEvidence::None);
    tokio::time::advance(Duration::from_secs(3600)).await;
    assert!(
        out.try_recv().is_err(),
        "acceptance is not permission and has no admission timer"
    );
    let mut progress = accepted.progress();
    let result = tokio::spawn(accepted.admit());
    is_control(out.recv().await.unwrap(), "admitted", &id);
    registry.unregister(connection, "r").unwrap();
    drop(registration);
    assert!(
        out.try_recv().is_err(),
        "invocation still pins retired registration"
    );
    broker
        .accept(
            connection,
            ClientFrame::Progress {
                invocation_id: id.clone(),
                current: 1,
                total: 3,
            },
        )
        .unwrap();
    progress.changed().await.unwrap();
    assert_eq!(progress.borrow().unwrap().current, 1);
    let result_value = json!({"content":[],"structuredContent":{"body":"界".repeat(16000)}});
    let bytes = serde_json::to_vec(&result_value).unwrap();
    broker
        .accept(
            connection,
            ClientFrame::ResultStart {
                invocation_id: id.clone(),
                byte_length: bytes.len() as u64,
                chunk_count: 2,
            },
        )
        .unwrap();
    assert!(
        broker
            .accept(
                Uuid::new_v4(),
                ClientFrame::ResultChunk {
                    invocation_id: id.clone(),
                    index: 0,
                    data: STANDARD.encode(&bytes[..36864])
                }
            )
            .is_err()
    );
    for (index, chunk) in bytes.chunks(36864).enumerate() {
        broker
            .accept(
                connection,
                ClientFrame::ResultChunk {
                    invocation_id: id.clone(),
                    index: index as u64,
                    data: STANDARD.encode(chunk),
                },
            )
            .unwrap();
    }
    assert_eq!(
        serde_json::to_value(result.await.unwrap().unwrap()).unwrap(),
        result_value
    );
    is_control(out.recv().await.unwrap(), "release", &id);
    assert!(
        matches!(out.recv().await.unwrap(),HostFrame::RegistrationRelease {registration_id} if registration_id=="r")
    );
    broker.accept(Uuid::new_v4(), provider_accept(&id)).unwrap(); // Late settled frames cannot revive work.
    assert!(
        broker
            .accept(connection, provider_accept("never-issued"))
            .is_err()
    );
    broker.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn cancellation_timeout_and_disconnect_are_known_only_before_admission() {
    let transport = CancellationToken::new();
    let (_registry, connection, registration, mut out) = setup(transport.clone());
    let registration_pins = Arc::strong_count(&registration);
    let broker = Broker::default();
    let cancellation = CancellationToken::new();
    let pending = broker
        .prepare_service(
            registration.clone(),
            ServiceCall {
                service_id: "service".into(),
                version: "1".into(),
                method: "perform".into(),
                input: serde_json::Map::new(),
            },
            Duration::from_secs(5),
            cancellation.clone(),
        )
        .unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let accepted = pending.accepted().await.unwrap();
    cancellation.cancel();
    // No scheduling yield: the authoritative cut must not depend on monitor polling.
    assert_eq!(accepted.admit().await.unwrap_err(), CallError::Cancelled);
    is_control(out.recv().await.unwrap(), "cancel", &id);
    is_control(out.recv().await.unwrap(), "release", &id);
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(6)).await;
    assert!(matches!(pending.accepted().await, Err(CallError::TimedOut)));
    is_control(out.recv().await.unwrap(), "cancel", &id);
    is_control(out.recv().await.unwrap(), "release", &id);
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let accepted = pending.accepted().await.unwrap();
    let result = tokio::spawn(accepted.admit());
    is_control(out.recv().await.unwrap(), "admitted", &id);
    tokio::time::advance(Duration::from_secs(6)).await;
    assert!(matches!(
        result.await.unwrap(),
        Err(CallError::OutcomeUnknown(_))
    ));
    is_control(out.recv().await.unwrap(), "cancel", &id);
    is_control(out.recv().await.unwrap(), "release", &id);
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let accepted = pending.accepted().await.unwrap();
    // Transport revocation must settle work even while an admitted ordinary
    // Host request keeps the connection lease attached to the registry.
    transport.cancel();
    assert_eq!(
        accepted.admit().await.unwrap_err(),
        CallError::CapabilityLost
    );
    assert!(
        out.try_recv().is_err(),
        "lost connection gets no acknowledgement"
    );
    broker.shutdown().await;
    assert_eq!(Arc::strong_count(&registration), registration_pins);
    let transport = CancellationToken::new();
    let (_registry, connection, registration, mut out) = setup(transport.clone());
    let registration_pins = Arc::strong_count(&registration);
    let broker = Broker::default();
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let result = tokio::spawn(pending.accepted().await.unwrap().admit());
    is_control(out.recv().await.unwrap(), "admitted", &id);
    transport.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), result)
            .await
            .expect("transport loss must settle before the five-second call deadline")
            .unwrap(),
        Err(CallError::OutcomeUnknown(_))
    ));
    assert!(out.try_recv().is_err());
    broker.shutdown().await;
    assert_eq!(Arc::strong_count(&registration), registration_pins);
}

#[tokio::test(start_paused = true)]
async fn dropped_waiters_release_capacity_and_shutdown_owns_every_call() {
    let (_registry, connection, registration, mut out) = setup(CancellationToken::new());
    let broker = Broker::default();
    let mut calls = Vec::new();
    for _ in 0..8 {
        calls.push(prepare(&broker, &registration).unwrap());
        out.recv().await.unwrap();
    }
    assert!(matches!(
        prepare(&broker, &registration),
        Err(CallError::Overloaded)
    ));
    let dropped = calls.pop().unwrap();
    let id = dropped.invocation_id().to_owned();
    drop(dropped);
    is_control(out.recv().await.unwrap(), "cancel", &id);
    is_control(out.recv().await.unwrap(), "release", &id);
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let accepted = pending.accepted().await.unwrap();
    let result = tokio::spawn(accepted.admit());
    is_control(out.recv().await.unwrap(), "admitted", &id);
    broker.shutdown().await;
    assert!(matches!(
        result.await.unwrap(),
        Err(CallError::OutcomeUnknown(_))
    ));
    for pending in calls {
        assert!(matches!(
            pending.accepted().await,
            Err(CallError::Cancelled)
        ));
    }
    assert!(matches!(
        prepare(&broker, &registration),
        Err(CallError::Cancelled)
    ));
    let broker = Broker::default();
    while out.try_recv().is_ok() {}
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let accepted = pending.accepted().await.unwrap();
    drop(broker);
    assert_eq!(accepted.admit().await.unwrap_err(), CallError::Cancelled);
    is_control(out.recv().await.unwrap(), "cancel", &id);
    is_control(out.recv().await.unwrap(), "release", &id);
}
