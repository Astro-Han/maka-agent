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

use super::support::{
    client_probe::ClientFixture,
    message_recovery::{Provider, configure},
    peer::Peer,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use maka_protocol::Operation;
use maka_runtime::{
    archive::ToolResultAddress,
    artifact::content_digest,
    event::{Fact, StoredEvent, ToolOutcome},
    read::ReadInput,
    tool_output::{DurableToolProjection, ToolOutput},
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resource_pages_survive_reopen_without_cross_session_access() {
    tokio::time::timeout(Duration::from_secs(30), scenario())
        .await
        .unwrap();
}

async fn scenario() {
    let fixture = ClientFixture::new("maka-read-page-");
    let (provider, mut requests) = Provider::controlled().await;
    let model = configure(&fixture, &provider.base_url).await;
    let source = "中😀A".repeat(2500);
    let mut next = Value::Null;
    let mut first_page = Value::Null;
    let mut prefix: Vec<StoredEvent> = Vec::new();
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("read.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-read-{}", uuid::Uuid::new_v4()));
        let cancel = CancellationToken::new();
        let guard = cancel.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), cancel),
        );
        let mut peer = Peer::new(host.clone(), "read-client").await;
        if !reopened {
            for session in ["reader", "foreign"] {
                let created = peer.rpc(Operation::SessionCreate.as_str(), json!({"sessionId":session,
                    "workspace":{"kind":"host_path","path":fixture.workspace},
                    "modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model},
                    "permissionMode":"explore","mode":"bot"})).await;
                assert_eq!(created["ok"], true, "{created}");
            }
            for input in [
                json!({"kind":"begin","sessionId":"reader","uploadId":"text","name":"source.txt","mimeType":"text/plain","totalBytes":source.len(),"contentSha256":content_digest(source.as_bytes())}),
                json!({"kind":"chunk","sessionId":"reader","uploadId":"text","offset":0,"chunkBase64":STANDARD.encode(source.as_bytes())}),
            ] {
                let response = peer.rpc(Operation::ArtifactIngest.as_str(), input).await;
                assert_eq!(response["ok"], true, "{response}");
            }
            let uploaded = peer
                .rpc(
                    Operation::ArtifactIngest.as_str(),
                    json!({"kind":"commit","sessionId":"reader","uploadId":"text"}),
                )
                .await;
            assert_eq!(uploaded["result"]["kind"], "committed", "{uploaded}");
            next = json!({"path":format!("maka://runtime/attachments/{}", uploaded["result"]["attachment"]["ref"]["relativePath"].as_str().unwrap())});
        }
        let turn = if reopened { "continue" } else { "initial" };
        start(&mut peer, "reader", turn).await;
        let request = requests.recv().await.unwrap();
        if reopened {
            assert_eq!(pages(&request.body), vec![first_page.clone()]);
        }
        let read = request.body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == "Read")
            .unwrap();
        assert_eq!(read["function"]["parameters"]["required"], json!(["path"]));
        assert!(
            read["function"]["parameters"]["properties"]
                .get("ref")
                .is_none()
        );
        let input: ReadInput = serde_json::from_value(next.clone()).unwrap();
        let expected =
            serde_json::to_value(input.resolve().unwrap().page(&source).unwrap()).unwrap();
        request.reply.send(call(next.clone())).unwrap();
        let response = requests.recv().await.unwrap();
        let values = pages(&response.body);
        assert_eq!(values.last(), Some(&expected));
        assert!(expected.to_string().encode_utf16().count() <= 7500);
        if !reopened {
            first_page = expected.clone();
            next = expected["next"].clone();
            assert!(!next.is_null());
        } else {
            assert_eq!(expected["next"], Value::Null);
            assert_eq!(
                format!(
                    "{}{}",
                    first_page["content"].as_str().unwrap(),
                    expected["content"].as_str().unwrap()
                ),
                source
            );
        }
        response.reply.send(done()).unwrap();
        finish(&mut peer, "reader", turn).await;
        if reopened {
            let event_id = &prefix
                .iter()
                .find(|stored| matches!(stored.event.fact, Fact::ToolSettled { .. }))
                .unwrap()
                .event
                .id;
            let event_input = json!({"path":ToolResultAddress::event_path(event_id).unwrap()});
            let expected_event_page = serde_json::to_value(
                serde_json::from_value::<ReadInput>(event_input.clone())
                    .unwrap()
                    .resolve()
                    .unwrap()
                    .page(&first_page.to_string())
                    .unwrap(),
            )
            .unwrap();
            start(&mut peer, "reader", "event-read").await;
            let request = requests.recv().await.unwrap();
            assert!(
                request.body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|tool| tool["function"]["name"] != "ArchiveRead")
            );
            request.reply.send(call(event_input.clone())).unwrap();
            let response = requests.recv().await.unwrap();
            assert_eq!(pages(&response.body).last(), Some(&expected_event_page));
            response.reply.send(done()).unwrap();
            finish(&mut peer, "reader", "event-read").await;
            start(&mut peer, "foreign", "denied").await;
            requests
                .recv()
                .await
                .unwrap()
                .reply
                .send(call(next.clone()))
                .unwrap();
            let response = requests.recv().await.unwrap();
            let message = response.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|message| message["role"] == "tool")
                .unwrap();
            assert_eq!(
                message["content"],
                "Attachment was not found in this Session"
            );
            response.reply.send(call(event_input)).unwrap();
            let response = requests.recv().await.unwrap();
            let denied: Value = serde_json::from_str(
                response.body["messages"]
                    .as_array()
                    .unwrap()
                    .last()
                    .unwrap()["content"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(denied["error"], "not_found");
            response.reply.send(done()).unwrap();
            finish(&mut peer, "foreign", "denied").await;
        }
        peer.close().await;
        drop(guard);
        server.await.unwrap().unwrap();
        drop(host);
        let log = fixture.log().await;
        let events = log.prefix(200, 1024 * 1024).await.unwrap().events;
        if reopened {
            assert_eq!(
                serde_json::to_vec(&events[..prefix.len()]).unwrap(),
                serde_json::to_vec(&prefix).unwrap()
            );
        }
        let mut reads = 0;
        for stored in &events {
            if let Fact::ToolSettled {
                outcome:
                    ToolOutcome::Succeeded {
                        model_projection, ..
                    },
                ..
            } = &stored.event.fact
            {
                if stored.event.invocation.turn_id == "event-read" {
                    assert!(matches!(
                        log.resolve_tool_result("reader", &stored.event.id)
                            .await
                            .unwrap(),
                        ToolOutput::Json(_)
                    ));
                    continue;
                }
                if stored.event.invocation.session_id == "foreign" {
                    assert!(
                        matches!(model_projection, DurableToolProjection::Json { value } if value["error"] == "not_found")
                    );
                    continue;
                }
                reads += 1;
                assert!(
                    matches!(model_projection, DurableToolProjection::Json { value } if value["content"].as_str().unwrap().len() < source.len())
                );
                assert_eq!(
                    log.resolve_tool_result("reader", &stored.event.id)
                        .await
                        .unwrap(),
                    ToolOutput::Text(source.clone())
                );
            }
        }
        assert_eq!(reads, if reopened { 2 } else { 1 });
        prefix = events;
        log.close().await.unwrap();
    }
}

fn pages(request: &Value) -> Vec<Value> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| serde_json::from_str(message["content"].as_str().unwrap()).unwrap())
        .collect()
}
fn call(input: Value) -> Value {
    json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":"read","type":"function","function":{"name":"Read","arguments":input.to_string()}}]},"finish_reason":"tool_calls"})
}
fn done() -> Value {
    json!({"index":0,"delta":{"content":"done"},"finish_reason":"stop"})
}
async fn start(peer: &mut Peer, session: &str, turn: &str) {
    let result = peer.rpc(Operation::TurnStart.as_str(), json!({"sessionId":session,"turnId":turn,"content":{"text":"Read the resource."},"maxSteps":3})).await;
    assert_eq!(result["ok"], true, "{result}");
}
async fn finish(peer: &mut Peer, session: &str, turn: &str) {
    loop {
        let result = peer
            .rpc(
                Operation::TurnQuery.as_str(),
                json!({"sessionId":session,"turnId":turn}),
            )
            .await;
        match result["result"]["status"].as_str() {
            Some("completed") => return,
            Some("failed" | "cancelled") => panic!("{result}"),
            _ => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
}
