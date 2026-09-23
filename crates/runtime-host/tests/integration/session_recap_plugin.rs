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

use super::{
    javascript_plugins::ready,
    support::{
        client_probe::ClientFixture,
        message_recovery::{Provider, configure},
        peer::Peer,
    },
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

async fn binding(peer: &mut Peer) -> Value {
    let snapshot = peer
        .rpc("plugin.client.query", json!({"kind":"snapshot"}))
        .await;
    let entry = snapshot["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == "maka.session-recap")
        .unwrap();
    json!({"client":{
        "entryId":entry["entryId"],"extensionId":entry["extensionId"],
        "activation":entry["activation"],"contentDigest":entry["contentDigest"],
        "clientDigest":entry["clientDigest"]
    },"method":"request","sessionId":"recap-session"})
}

async fn open(peer: &mut Peer) -> Value {
    let binding = binding(peer).await;
    let bound = peer
        .rpc("plugin.remote", json!({"kind":"bind","binding":binding}))
        .await;
    assert_eq!(bound["ok"], true, "{bound}");
    let opened = peer
        .rpc("plugin.remote", json!({"kind":"open_document"}))
        .await;
    assert_eq!(opened["ok"], true, "{opened}");
    json!({"kind":"call","document":opened["result"]["document"],
        "binding":binding,"target":bound["result"]["target"]})
}
async fn call(peer: &mut Peer, envelope: &Value, input: Value) -> Value {
    let mut call = envelope.clone();
    call["input"] = input;
    peer.rpc("plugin.remote", call).await
}
async fn close(peer: &mut Peer, envelope: &Value) {
    let response = peer
        .rpc(
            "plugin.remote",
            json!({
                "kind":"close_document","document":envelope["document"]
            }),
        )
        .await;
    assert_eq!(response["ok"], true, "{response}");
}
async fn disable(peer: &mut Peer, disabled: bool) {
    let result = peer
        .rpc(
            "plugin.composition.apply",
            json!({"operations":[
                {"type":"update","entryId":"maka.session-recap","patch":{"disabled":disabled}}
            ]}),
        )
        .await;
    assert_eq!(result["ok"], true, "{result}");
    if !disabled {
        ready(peer).await;
    } else {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let clients = peer
                    .rpc("plugin.client.query", json!({"kind":"snapshot"}))
                    .await;
                if !clients["result"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["extensionId"] == "maka.session-recap")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn recap_uses_authorized_history_and_model_and_persists_across_host_restart() {
    tokio::time::timeout(Duration::from_secs(60), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-session-recap-");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&fixture.workspace)
            .status()
            .unwrap()
            .success()
    );
    let provider = Provider::start().await;
    let model = configure(&fixture, &provider.base_url).await;
    let operation_id = uuid::Uuid::new_v4();
    let mut saved = Value::Null;
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("recap.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-recap-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop),
        );
        let mut peer = Peer::new(host, "recap").await;
        ready(&mut peer).await;
        if !reopened {
            let created=peer.rpc("session.create",json!({"sessionId":"recap-session","name":"Recap fixture",
                "workspace":{"kind":"host_path","path":fixture.workspace},"sandboxMode":"danger-full-access",
                "modelTarget":{"kind":"explicit","connectionId":model.connection_id,
                "connectionSlug":model.connection_slug,"model":model.model}})).await;
            assert_eq!(created["ok"], true, "{created}");
            let started = peer
                .rpc(
                    "turn.start",
                    json!({"sessionId":"recap-session","turnId":"source-turn",
                "content":{"text":"Tests passed; deployment is still pending."}}),
                )
                .await;
            assert_eq!(started["ok"], true, "{started}");
            loop {
                let state = peer
                    .rpc(
                        "turn.query",
                        json!({"sessionId":"recap-session","turnId":"source-turn"}),
                    )
                    .await;
                assert_eq!(state["ok"], true, "{state}");
                if state["result"]["status"] == "completed" {
                    break;
                }
                assert!(
                    matches!(
                        state["result"]["status"].as_str(),
                        Some("admitted" | "created" | "running")
                    ),
                    "{state}"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        let envelope = open(&mut peer).await;
        let read = call(&mut peer, &envelope, json!({"kind":"read"})).await;
        assert_eq!(read["ok"], true, "{read}");
        if reopened {
            assert_eq!(read["result"]["value"]["recap"], saved);
        } else {
            assert!(read["result"]["value"]["recap"].is_null());
        }
        let generated = call(
            &mut peer,
            &envelope,
            json!({"kind":"generate","operationId":operation_id}),
        )
        .await;
        assert_eq!(generated["ok"], true, "{generated}");
        let recap = generated["result"]["value"]["recap"].clone();
        assert_eq!(recap["kind"], "ready", "{recap}");
        assert_eq!(recap["text"], "recovered");
        if reopened {
            assert_eq!(recap, saved);
        } else {
            saved = recap;
        }
        let duplicate = call(
            &mut peer,
            &envelope,
            json!({"kind":"generate","operationId":operation_id}),
        )
        .await;
        assert_eq!(duplicate["result"]["value"]["recap"], saved);
        let count = provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| {
                r.to_string()
                    .contains("The user is returning to this session")
            })
            .count();
        assert_eq!(
            count, 1,
            "recap retries must not dispatch another model call"
        );
        disable(&mut peer, true).await;
        let retired = call(&mut peer, &envelope, json!({"kind":"read"})).await;
        assert_eq!(retired["ok"], false);
        disable(&mut peer, false).await;
        close(&mut peer, &envelope).await;
        peer.close().await;
        drop(cleanup);
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
