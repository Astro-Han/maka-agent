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
fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":input.to_string()}}]},"finish_reason":"tool_calls"})
}
fn result(body: &Value, id: &str) -> Value {
    let message = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == id)
        .unwrap_or_else(|| panic!("missing {id}: {body}"));
    serde_json::from_str(message["content"].as_str().unwrap())
        .unwrap_or_else(|_| panic!("not JSON: {message}"))
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn health_reads_real_shell_state_through_agent_authority() {
    tokio::time::timeout(Duration::from_secs(60), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-background-health-");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&fixture.workspace)
            .status()
            .unwrap()
            .success()
    );
    let (provider, mut requests) = Provider::controlled().await;
    let model = configure(&fixture, &provider.base_url).await;
    let host = Host::open(fixture.owner()).await.unwrap();
    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("health.sock");
    #[cfg(windows)]
    let endpoint =
        std::path::PathBuf::from(format!(r"\\.\pipe\maka-health-{}", uuid::Uuid::new_v4()));
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop),
    );
    let mut peer = Peer::new(host, "health").await;
    ready(&mut peer).await;
    for session in ["health-owner", "health-other"] {
        let created=peer.rpc("session.create",json!({"sessionId":session,"workspace":{"kind":"host_path","path":fixture.workspace},"sandboxMode":"danger-full-access","approvalPolicy":{"kind":"never"},"modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model}})).await;
        assert_eq!(created["ok"], true, "{created}");
    }
    let started=peer.rpc("turn.start",json!({"sessionId":"health-owner","turnId":"health-turn","content":{"text":"Start and inspect a background task."}})).await;
    assert_eq!(started["ok"], true, "{started}");
    let request = requests.recv().await.unwrap();
    request
        .reply
        .send(tool("search", "tool_search", json!({"query":"Shell Read"})))
        .unwrap();
    let request = requests.recv().await.unwrap();
    request
        .reply
        .send(tool(
            "shell",
            "Shell",
            json!({"command":"echo health-log","login":false,"run_in_background":true}),
        ))
        .unwrap();
    let request = requests.recv().await.unwrap();
    let shell = result(&request.body, "shell");
    let reference = shell["ref"].as_str().unwrap_or_else(|| panic!("{shell}"));
    request
        .reply
        .send(tool(
            "health",
            "BackgroundTaskHealth",
            json!({"ref":reference}),
        ))
        .unwrap();
    let request = requests.recv().await.unwrap();
    let health = result(&request.body, "health");
    assert_eq!(health["process"]["tracked"], true, "{health}");
    assert!(health["process"].get("logs").is_none(), "{health}");
    assert_eq!(health["endpoint"]["state"], "not_checked", "{health}");
    request
        .reply
        .send(tool(
            "logs",
            "BackgroundTaskHealth",
            json!({"ref":reference,"include_logs":true}),
        ))
        .unwrap();
    let request = requests.recv().await.unwrap();
    let logs = result(&request.body, "logs");
    assert!(logs["process"]["logs"].is_object(), "{logs}");
    request
        .reply
        .send(json!({"index":0,"delta":{"content":"checked"},"finish_reason":"stop"}))
        .unwrap();
    let started=peer.rpc("turn.start",json!({"sessionId":"health-other","turnId":"other-turn","content":{"text":"Inspect the supplied task reference."}})).await;
    assert_eq!(started["ok"], true, "{started}");
    let request = requests.recv().await.unwrap();
    request
        .reply
        .send(tool(
            "foreign",
            "BackgroundTaskHealth",
            json!({"ref":reference}),
        ))
        .unwrap();
    let request = requests.recv().await.unwrap();
    let foreign = request.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == "foreign")
        .unwrap();
    assert!(
        foreign["content"]
            .as_str()
            .unwrap()
            .contains("Runtime background task not found in this session"),
        "{foreign}"
    );
    request
        .reply
        .send(json!({"index":0,"delta":{"content":"denied"},"finish_reason":"stop"}))
        .unwrap();
    loop {
        let state = peer
            .rpc(
                "turn.query",
                json!({"sessionId":"health-other","turnId":"other-turn"}),
            )
            .await;
        if state["result"]["status"] == "completed" {
            break;
        }
        assert!(
            matches!(
                state["result"]["status"].as_str(),
                Some("created" | "admitted" | "running")
            ),
            "{state}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    peer.close().await;
    drop(cleanup);
    server.await.unwrap().unwrap();
}
