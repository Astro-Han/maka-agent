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
    broker::{Broker, ServiceCall, ToolCall},
};
use maka_protocol::capability::{
    decode_client_frame, decode_replace_input, decode_unregister_input,
};
use maka_runtime::capability::AdmissionEvidence;
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// This integrates the current-source ClientCapabilityChannel with Registry and
// Broker. The private line bridge does not claim installed Host operations.
#[tokio::test]
async fn original_client_service_and_tool_admission_chunks_and_release() {
    tokio::time::timeout(Duration::from_secs(30), service_roundtrip())
        .await
        .expect("original-client service test exceeded its subprocess deadline");
}

async fn service_roundtrip() {
    let workspace = tempfile::tempdir().unwrap();
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs"))
        .arg("--capability-service-workspace")
        .arg(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("start original client source probe");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (endpoint, mut host_frames) = Endpoint::channel(32);
    let connection = Uuid::new_v4();
    let registry = Arc::new(Mutex::new(Registry::default()));
    let provider_id = registry
        .lock()
        .unwrap()
        .attach(
            connection,
            Identity {
                principal_kind: PrincipalKind::LocalOwner,
                principal_id: "service-interop-owner".into(),
                client_instance_id: "service-interop-client".into(),
                credential_bound_client_instance_id: None,
                capability_owner: None,
            },
            endpoint.clone(),
        )
        .unwrap();
    let broker = Arc::new(Broker::default());
    let (outgoing, mut messages) = mpsc::channel::<Value>(32);
    let (events, mut incoming) = mpsc::channel::<Value>(32);
    // JoinSet owns both bridge tasks, aborting them even when an assertion fails.
    let mut bridge = JoinSet::new();
    bridge.spawn(async move {
        loop {
            let value = tokio::select! {
                // Registry release precedes the mutation response/control fence.
                biased;
                Some(frame) = host_frames.recv() => json!({"kind":"host", "frame":frame}),
                Some(message) = messages.recv() => message,
                else => break,
            };
            let mut encoded = serde_json::to_vec(&value).unwrap();
            encoded.push(b'\n');
            stdin.write_all(&encoded).await.unwrap();
        }
    });
    let reader_registry = registry.clone();
    let reader_broker = broker.clone();
    let replies = outgoing.clone();
    bridge.spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await.unwrap() {
            let message: Value = serde_json::from_str(&line).unwrap();
            match message["kind"].as_str() {
                Some("replace") => {
                    let manifest = decode_replace_input(&message["input"]).unwrap();
                    assert_eq!(manifest.offers.len(), 2);
                    assert_eq!(manifest.services.as_ref().unwrap().len(), 1);
                    let result = reader_registry
                        .lock()
                        .unwrap()
                        .replace(connection, manifest)
                        .unwrap();
                    replies
                        .send(json!({"kind":"reply", "id":message["id"], "result":result}))
                        .await
                        .unwrap();
                }
                Some("unregister") => {
                    let input = decode_unregister_input(&message["input"]).unwrap();
                    let result = reader_registry
                        .lock()
                        .unwrap()
                        .unregister(connection, &input.registration_id)
                        .unwrap();
                    replies
                        .send(json!({"kind":"reply", "id":message["id"], "result":result}))
                        .await
                        .unwrap();
                }
                Some("frame") => {
                    let frame = decode_client_frame(&message["frame"]).unwrap();
                    reader_broker.accept(connection, frame).unwrap();
                }
                Some(_) => events.send(message).await.unwrap(),
                None => assert_eq!(message["check"], "current-source-build"),
            }
        }
    });

    let workflow = async {
        expect_event(&mut incoming, "ready").await;
        let registration = registry.lock().unwrap().current(&provider_id).unwrap();
        let pinned = Arc::downgrade(&registration);
        let pending = broker
            .prepare_service(
                registration,
                ServiceCall {
                    service_id: "test_effect".into(),
                    version: "1".into(),
                    method: "write".into(),
                    input: serde_json::from_value(json!({"text":"admitted effect"})).unwrap(),
                },
                Duration::from_secs(5),
                CancellationToken::new(),
            )
            .unwrap();
        let accepted = pending.accepted().await.unwrap();
        assert_eq!(accepted.evidence(), &AdmissionEvidence::None);
        // A round trip through the provider event loop fences its acceptance. The
        // provider must still be suspended at await accept(), without a file effect.
        outgoing.send(json!({"kind":"checkpoint"})).await.unwrap();
        expect_event(&mut incoming, "checkpoint").await;
        assert!(!workspace.path().join("provider-effect.txt").exists());
        let result = accepted.admit().await.unwrap();
        assert!(result.content.is_empty());
        assert_eq!(
            result.structured_content.unwrap(),
            json!({
                "text":"é".repeat(40000), "effect":"admitted effect",
            })
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("provider-effect.txt")).unwrap(),
            "admitted effect"
        );

        for (index, access) in ["none", "cwd"].into_iter().enumerate() {
            let registration = registry.lock().unwrap().current(&provider_id).unwrap();
            let registration_id = registration.manifest().registration_id.clone();
            let pending = broker
                .prepare_tool(
                    registration,
                    ToolCall {
                        offer_id: access.into(),
                        server_id: format!("test-server-{access}"),
                        tool_name: "write".into(),
                        arguments: serde_json::from_value(json!({"text": access})).unwrap(),
                        source: if index == 0 {
                            maka_runtime::capability::CallSource::Agent {
                                session_id: "interop-session".into(),
                                turn_id: "interop-turn".into(),
                            }
                        } else {
                            maka_runtime::capability::CallSource::Background {
                                session_id: None,
                                grant_id: "authorized-grant".into(),
                            }
                        },
                        tool_call_id: format!("interop-call-{access}"),
                        cwd: workspace.path().to_str().unwrap().into(),
                    },
                    Duration::from_secs(5),
                    CancellationToken::new(),
                )
                .unwrap();
            let accepted = pending.accepted().await.unwrap();
            assert_eq!(accepted.evidence(), &AdmissionEvidence::None);
            if index == 0 {
                outgoing.send(json!({"kind":"replace"})).await.unwrap();
                expect_event(&mut incoming, "replaced").await;
                assert_ne!(
                    registry
                        .lock()
                        .unwrap()
                        .current(&provider_id)
                        .unwrap()
                        .manifest()
                        .registration_id,
                    registration_id
                );
                assert!(
                    pinned.upgrade().is_some(),
                    "accepted call lost its registration"
                );
            }
            outgoing
                .send(json!({"kind":"tool-checkpoint", "access":access}))
                .await
                .unwrap();
            expect_event(&mut incoming, "tool-checkpoint").await;
            let effect = workspace.path().join(format!("tool-{access}.txt"));
            assert!(!effect.exists());
            let result = accepted.admit().await.unwrap();
            assert_eq!(
                serde_json::to_value(result.content).unwrap(),
                json!([{"type":"text", "text":access}])
            );
            assert_eq!(
                result.structured_content.unwrap(),
                json!({
                    "generation":index + 1, "registrationId":registration_id,
                })
            );
            assert_eq!(std::fs::read_to_string(effect).unwrap(), access);
            assert!(
                pinned.upgrade().is_none(),
                "completed old registration remains pinned"
            );
        }

        let pinned = Arc::downgrade(&registry.lock().unwrap().current(&provider_id).unwrap());
        outgoing.send(json!({"kind":"unregister"})).await.unwrap();
        expect_event(&mut incoming, "unregistered").await;
        assert!(registry.lock().unwrap().current(&provider_id).is_none());
        assert!(
            pinned.upgrade().is_none(),
            "finished invocation retained its registration"
        );
        broker.shutdown().await;
        registry.lock().unwrap().begin_drain();
        assert!(endpoint.closed().is_cancelled());
        assert!(endpoint.invocations().is_cancelled());
        outgoing.send(json!({"kind":"finish"})).await.unwrap();
        let done = expect_event(&mut incoming, "done").await;
        assert!(done["resultChunks"].as_u64().unwrap() > 1);
        assert!(child.wait().await.unwrap().success());
    };
    tokio::pin!(workflow);
    loop {
        tokio::select! {
            () = &mut workflow => break,
            Some(result) = bridge.join_next() => result.expect("client bridge task failed"),
        }
    }
    bridge.abort_all();
    while let Some(result) = bridge.join_next().await {
        if let Err(error) = result {
            assert!(error.is_cancelled(), "test bridge failed: {error}");
        }
    }
}

async fn expect_event(events: &mut mpsc::Receiver<Value>, kind: &str) -> Value {
    let event = events
        .recv()
        .await
        .expect("client bridge closed prematurely");
    assert_eq!(event["kind"], kind);
    event
}
