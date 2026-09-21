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

use super::{package, ready};
use crate::support::{
    client_probe::ClientFixture, message_recovery::configure_provider, peer::Peer,
};
use maka_plugins::{
    composition::Scope,
    execution::{Progress, Submit},
    fiber::Fiber,
};
use maka_runtime::event::InvocationOutcome;
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

const PLUGIN: &str = r#"
export default function(ctx) {
    ctx.tools.bind([{
        name: 'Research', description: 'Search reference documents.',
        inputSchema: {type:'object', properties:{query:{type:'string'}}},
        alwaysVisible: true,
    }], async ({model}) => {
        if (model.providerTools !== 'anthropic_messages') throw new Error('wrong model surface');
        return {providerTools: {Research: {
            id:'anthropic.web_search_20250305', args:{maxUses:2},
        }}};
    });
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_provider_bindings_cross_both_vm_modes_without_local_handlers() {
    tokio::time::timeout(Duration::from_secs(30), scenario())
        .await
        .unwrap();
}

async fn scenario() {
    for mode in ["shared", "dedicated"] {
        let fixture = ClientFixture::new("maka-provider-plugin-");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/", listener.local_addr().unwrap());
        let model = configure_provider(&fixture, &base_url, "anthropic-compatible").await;
        let path = package(&fixture.workspace, "example.research", mode, PLUGIN, false);
        let provider = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let body = loop {
                let mut chunk = [0u8; 8192];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0 && bytes.len() + n <= 1024 * 1024);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&bytes[..offset]).unwrap();
                    let length = header
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= offset + 4 + length {
                        break serde_json::from_slice::<Value>(
                            &bytes[offset + 4..offset + 4 + length],
                        )
                        .unwrap();
                    }
                }
            };
            assert!(
                body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["type"] == "web_search_20250305" && tool["max_uses"] == 2),
                "{body}"
            );
            assert!(
                !body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["name"] == "Research")
            );
            let events = [
                json!({"type":"message_start","message":{"id":"msg_fixture","type":"message","role":"assistant","model":"fixture-model","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Search binding accepted."}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":4}}),
                json!({"type":"message_stop"}),
            ];
            let body: String = events
                .iter()
                .map(|event| {
                    format!(
                        "event: {}\ndata: {event}\n\n",
                        event["type"].as_str().unwrap()
                    )
                })
                .collect();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        });
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("p.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-provider-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop),
        );
        let mut peer = Peer::new(host.clone(), "provider-plugin").await;
        let installed = peer
            .rpc("plugin.package.install", json!({"sourcePath":path}))
            .await;
        assert_eq!(installed["ok"], true, "{installed}");
        ready(&mut peer).await;
        let created = peer.rpc("session.create", json!({
                "sessionId":"research", "workspace":{"kind":"host_path","path":fixture.workspace},
                "permissionMode":"bypass",
                "modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model}
            })).await;
        assert_eq!(created["ok"], true, "{created}");
        let driver = Fiber::new("example.driver", "driver", Scope::Profile).unwrap();
        driver.begin_loading().unwrap();
        let commands = host
            .authorize_plugin_execution(driver.context(), &["research".into()])
            .await
            .unwrap();
        driver.ready().unwrap();
        driver.publish().unwrap();
        commands
            .submit(Submit {
                orchestration_mode: None,
                operation_id: "research".into(),
                session_id: "research".into(),
                content: "Search.".into(),
            })
            .await
            .unwrap();
        loop {
            if let Progress::Ended { outcome } =
                commands.query("research".into()).await.unwrap().progress
            {
                assert_eq!(outcome, InvocationOutcome::Completed);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        provider.await.unwrap();
        driver
            .shutdown(tokio::time::Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
        peer.close().await;
        drop(cleanup);
        server.await.unwrap().unwrap();
    }
}
