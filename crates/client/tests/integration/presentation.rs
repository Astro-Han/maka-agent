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
use maka_client::{Client, ClientError, Notification, OAuthPresentationService, RequestFailure};
use maka_protocol::Operation;
use maka_transport::ndjson::{NdjsonReader, NdjsonWriter};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

mod broker;

struct Fixture {
    client: Client,
    notices: tokio::sync::mpsc::Receiver<Notification>,
    service: OAuthPresentationService,
    reader: NdjsonReader<ReadHalf<DuplexStream>>,
    writer: NdjsonWriter<WriteHalf<DuplexStream>>,
}

impl Fixture {
    async fn new() -> Self {
        let (client, notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let publishing = tokio::spawn({
            let client = client.clone();
            async move { client.publish_oauth_presentation().await }
        });
        let frame = reader.read().await.unwrap().unwrap();
        assert_eq!(frame["operation"], "client.capability.replace");
        assert_eq!(frame["input"]["offers"], json!([]));
        assert_eq!(
            frame["input"]["services"],
            json!([{"serviceId":"oauth_presentation","version":"1"}])
        );
        writer
            .write(
                &json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,
            "result":{"registrationId":frame["input"]["registrationId"],"revision":1}}),
            )
            .await
            .unwrap();
        let service = publishing.await.unwrap().unwrap();
        assert_eq!(service.registration_id, frame["input"]["registrationId"]);
        Self {
            client,
            notices,
            service,
            reader,
            writer,
        }
    }

    fn call(&self, id: &str) -> Value {
        json!({"kind":"client.capability.service_call","registrationId":self.service.registration_id,
            "invocationId":id,"serviceId":"oauth_presentation","version":"1","method":"open_external",
            "input":{"url":"https://login.example/device","stateHint":"ABCD-1234"}})
    }

    async fn barrier(&mut self) {
        let request = tokio::spawn({
            let client = self.client.clone();
            async move { client.request(Operation::HostWake, json!({})).await }
        });
        let frame = self.reader.read().await.unwrap().unwrap();
        assert_eq!(
            frame["operation"], "host.wake",
            "unexpected outgoing frame: {frame}"
        );
        self.writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":{}})).await.unwrap();
        request.await.unwrap().unwrap();
    }

    async fn admit(&mut self, id: &str) -> maka_client::OAuthPresentation {
        self.writer.write(&self.call(id)).await.unwrap();
        let accepted = self.reader.read().await.unwrap().unwrap();
        maka_protocol::capability::decode_client_frame(&accepted).unwrap();
        assert_eq!(
            accepted,
            json!({"kind":"client.capability.accepted","invocationId":id,"admissionEvidence":{"kind":"none"}})
        );
        self.writer
            .write(&json!({"kind":"client.capability.admitted","invocationId":id}))
            .await
            .unwrap();
        self.service.recv().await.unwrap()
    }

    async fn release(&mut self, id: &str) {
        self.writer
            .write(&json!({"kind":"client.capability.release","invocationId":id}))
            .await
            .unwrap();
        self.barrier().await;
    }
}

#[tokio::test]
async fn presentation_waits_for_admission_and_display_without_blocking_rpc_and_cancellation() {
    let mut fixture = Fixture::new().await;
    fixture.writer.write(&fixture.call("first")).await.unwrap();
    assert_eq!(
        fixture.reader.read().await.unwrap().unwrap()["kind"],
        "client.capability.accepted"
    );
    // Acceptance has been processed, but nothing may reach the UI before admission.
    tokio::select! {
        biased;
        _ = fixture.service.recv() => panic!("presented before Host admission"),
        _ = std::future::ready(()) => {}
    }
    fixture
        .writer
        .write(&json!({"kind":"connection.catalog.changed","revision":5}))
        .await
        .unwrap();
    assert!(matches!(
        fixture.notices.recv().await.unwrap(),
        Notification::Catalog(_)
    ));
    fixture.barrier().await;
    fixture
        .writer
        .write(&json!({"kind":"client.capability.admitted","invocationId":"first"}))
        .await
        .unwrap();
    let shown = fixture.service.recv().await.unwrap();
    assert_eq!(shown.url, "https://login.example/device");
    assert_eq!(shown.state_hint.as_deref(), Some("ABCD-1234"));
    fixture.barrier().await; // No result until the actual UI acknowledges display.
    assert!(shown.acknowledge_presented());
    let result = fixture.reader.read().await.unwrap().unwrap();
    maka_protocol::capability::decode_client_frame(&result).unwrap();
    assert_eq!(
        result,
        json!({"kind":"client.capability.result","invocationId":"first",
        "result":{"content":[],"structuredContent":{"kind":"presented"}}})
    );
    fixture.release("first").await;

    let abandoned = fixture.admit("abandoned").await;
    drop(abandoned);
    let failed = fixture.reader.read().await.unwrap().unwrap();
    maka_protocol::capability::decode_client_frame(&failed).unwrap();
    assert_eq!(failed["kind"], "client.capability.failed");
    assert!(!failed.to_string().contains("ABCD"));
    fixture.release("abandoned").await;

    let cancelled = fixture.admit("cancelled").await;
    fixture
        .writer
        .write(&json!({"kind":"client.capability.cancel","invocationId":"cancelled"}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), cancelled.cancelled())
        .await
        .unwrap();
    assert!(!cancelled.acknowledge_presented());
    fixture.barrier().await; // Late local completion emits no result after cancel.
    fixture.release("cancelled").await;

    // A UI that has not dequeued the request yet must not see a cancelled code
    // later, even when release and registration release arrive in the meantime.
    fixture.writer.write(&fixture.call("queued")).await.unwrap();
    assert_eq!(
        fixture.reader.read().await.unwrap().unwrap()["kind"],
        "client.capability.accepted"
    );
    fixture
        .writer
        .write(&json!({"kind":"client.capability.admitted","invocationId":"queued"}))
        .await
        .unwrap();
    fixture
        .writer
        .write(&json!({"kind":"client.capability.cancel","invocationId":"queued"}))
        .await
        .unwrap();
    fixture.release("queued").await;

    fixture.writer.write(&json!({"kind":"client.capability.registration_release","registrationId":fixture.service.registration_id})).await.unwrap();
    assert!(fixture.service.recv().await.is_none());
    fixture.barrier().await; // Release retires the service, not the whole connection.
    fixture.client.disconnect();
}

#[tokio::test]
async fn unsafe_presentations_are_rejected_and_invalid_ownership_or_order_closes() {
    let mut fixture = Fixture::new().await;
    for (field, value) in [
        ("url", "http://login.example/device"),
        ("url", "javascript:alert(1)"),
        ("url", "https://user:secret@login.example/device"),
        ("url", "https://login.example/\u{001b}device"),
        ("serviceId", "unpublished_service"),
        ("version", "2"),
        ("method", "run_shell"),
    ] {
        let mut call = fixture.call("rejected");
        if field == "url" {
            call["input"][field] = json!(value);
        } else {
            call[field] = json!(value);
        }
        fixture.writer.write(&call).await.unwrap();
        let rejected = fixture.reader.read().await.unwrap().unwrap();
        assert_eq!(
            rejected,
            json!({"kind":"client.capability.rejected","invocationId":"rejected","message":"Unsupported OAuth presentation"})
        );
        fixture.release("rejected").await;
    }
    drop(fixture.service);
    tokio::time::timeout(Duration::from_secs(1), fixture.client.closed())
        .await
        .unwrap();

    for violation in ["registration", "invocation", "admission", "concurrent"] {
        let mut fixture = Fixture::new().await;
        let frame = match violation {
            "registration" => {
                let mut call = fixture.call("first");
                call["registrationId"] = json!("other-registration");
                call
            }
            "invocation" => json!({"kind":"client.capability.cancel","invocationId":"unknown"}),
            "admission" => json!({"kind":"client.capability.admitted","invocationId":"unknown"}),
            "concurrent" => {
                fixture.writer.write(&fixture.call("first")).await.unwrap();
                fixture.reader.read().await.unwrap();
                fixture.call("second")
            }
            _ => unreachable!(),
        };
        fixture.writer.write(&frame).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), fixture.client.closed())
                .await
                .unwrap(),
            ClientError::Protocol(_)
        ));
    }
}

#[tokio::test]
async fn publication_binds_receipt_and_disconnect_revokes_an_admitted_request() {
    let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
    assert!(matches!(client.request(Operation::ClientCapabilityReplace,
        json!({"registrationId":"raw","offers":[],"services":[{"serviceId":"oauth_presentation","version":"1"}]})).await,
        Err(RequestFailure::NotDispatched(_))));
    let publishing = tokio::spawn({
        let client = client.clone();
        async move { client.publish_oauth_presentation().await }
    });
    let frame = reader.read().await.unwrap().unwrap();
    writer
        .write(
            &json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,
        "result":{"registrationId":"wrong-registration","revision":1}}),
        )
        .await
        .unwrap();
    assert!(matches!(
        publishing.await.unwrap(),
        Err(RequestFailure::Unknown(ClientError::Protocol(_)))
    ));
    tokio::time::timeout(Duration::from_secs(1), client.closed())
        .await
        .unwrap();

    let mut fixture = Fixture::new().await;
    let call = fixture.admit("disconnect").await;
    fixture.client.disconnect();
    tokio::time::timeout(Duration::from_secs(1), call.cancelled())
        .await
        .unwrap();
    assert!(!call.acknowledge_presented());
}
