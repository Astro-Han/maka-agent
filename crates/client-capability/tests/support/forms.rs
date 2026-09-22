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

use maka_client_capability::{
    Endpoint, Identity, PrincipalKind, Registry,
    broker::{Broker, CallError, FormFuture, FormHandler, PendingCall, ServiceCall},
};
use maka_runtime::capability::{
    AdmissionEvidence, ClientFrame, FormInput, FormRequester, FormResult, HostFrame, Manifest,
    ServiceOffer,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub fn setup(
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
                session_id: None,
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
pub fn prepare(
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
pub fn provider_accept(id: &str) -> ClientFrame {
    ClientFrame::Accepted {
        invocation_id: id.into(),
        admission_evidence: AdmissionEvidence::None,
    }
}
pub fn is_control(frame: HostFrame, kind: &str, id: &str) {
    assert_eq!(
        serde_json::to_value(frame).unwrap(),
        json!({"kind":format!("client.capability.{kind}"),"invocationId":id})
    );
}

pub struct Request {
    pub cancellation: CancellationToken,
    pub answer: oneshot::Sender<FormResult>,
    pub withdrawn: oneshot::Sender<()>,
}
pub struct Handler(pub mpsc::UnboundedSender<Request>);
impl FormHandler for Handler {
    fn request(&self, _: FormInput, cancellation: CancellationToken) -> FormFuture {
        let (answer, answered) = oneshot::channel();
        let (withdrawn, withdrawal) = oneshot::channel();
        self.0
            .send(Request {
                cancellation: cancellation.clone(),
                answer,
                withdrawn,
            })
            .unwrap();
        Box::pin(async move {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    let _ = withdrawal.await;
                    Err(CallError::Cancelled)
                }
                answer = answered => Ok(answer.unwrap()),
            }
        })
    }
}
pub fn form(id: &str, interaction: &str) -> ClientFrame {
    ClientFrame::InteractionRequest {
        invocation_id: id.into(),
        interaction_id: interaction.into(),
        request: FormInput {
            message: "Fill form".into(),
            requester: FormRequester {
                name: "Provider".into(),
                source: None,
            },
            fields: vec![],
        },
    }
}
pub fn final_result(id: &str) -> ClientFrame {
    ClientFrame::Result {
        invocation_id: id.into(),
        result: maka_runtime::capability::CallResult {
            content: vec![],
            structured_content: None,
        },
    }
}
