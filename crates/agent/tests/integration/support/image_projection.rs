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

use maka_agent::{Engine, RunInput};
use maka_event_log::EventLog;
use maka_js_runtime::{CellLimits, CodeExecutor};
use maka_model::{ModelExecutor, ProviderConfig, ProviderKind};
use maka_runtime::{
    artifact::{Artifact, ArtifactKind, ArtifactSource, content_digest},
    event::Invocation,
};
use maka_tools::ToolCatalog;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use super::invocation;

pub fn identity(suffix: &str) -> Invocation {
    Invocation {
        session_id: "session".into(),
        turn_id: format!("turn-{suffix}"),
        run_id: format!("run-{suffix}"),
        invocation_id: format!("invocation-{suffix}"),
    }
}

pub fn engine(log: Arc<EventLog>) -> Engine {
    Engine::new(
        log,
        ModelExecutor::new(1, Duration::from_secs(30)).unwrap(),
        CodeExecutor::new(1, CellLimits::default()).unwrap(),
    )
}

pub fn input(base: &str, attachments: Value) -> RunInput {
    RunInput {
        main_output_limit: None,
        context: None,
        invocation: identity("current"),
        request_fingerprint: None,
        provider: ProviderConfig {
            kind: ProviderKind::OpenaiChat,
            model: "test".into(),
            base_url: base.into(),
            auth: maka_model::ProviderAuth::ApiKey("fixture".into()),
            headers: BTreeMap::new(),
            network: Default::default(),
            body_overlay: None,
        },
        provider_options: json!({}),
        supports_vision: true,
        configuration: invocation::configuration(maka_tools::ToolMode::Direct),
        work: maka_agent::RunWork::Message {
            skill_invocation: Default::default(),
            source_messages: Vec::new(),
            message: serde_json::from_value(json!({"text":"current", "attachments":attachments}))
                .unwrap(),
            tools: ToolCatalog::new([]).unwrap(),
            max_steps: 1,
        },
    }
}

pub fn attachment(id: &str) -> Value {
    // Deliberately unrelated to real payload length: budgeting must use storage bytes.
    json!({"kind":"image", "name":id, "mimeType":"image/png", "bytes":1,
        "ref":{"kind":"session_file", "sessionId":"session", "relativePath":id}})
}

pub async fn artifact(log: &EventLog, id: &str, size: usize, valid: bool) {
    let mut bytes = vec![0; size];
    if valid {
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    }
    log.commit_artifact(
        Artifact {
            id: id.into(),
            session_id: "session".into(),
            turn_id: "upload".into(),
            created_at: 1,
            name: id.into(),
            kind: ArtifactKind::Image,
            size_bytes: size as u64,
            mime_type: Some("image/png".into()),
            source: ArtifactSource::UserUpload,
            summary: Some(content_digest(&bytes)),
        },
        bytes,
    )
    .await
    .unwrap();
}

pub async fn request(socket: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let boundary = loop {
        let mut chunk = [0; 8192];
        let count = socket.read(&mut chunk).await.unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        assert!(bytes.len() < 64 * 1024);
    };
    let length: usize = String::from_utf8_lossy(&bytes[..boundary])
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    assert!(length < 20 * 1024 * 1024);
    let received = bytes.len();
    bytes.resize(boundary + length, 0);
    socket.read_exact(&mut bytes[received..]).await.unwrap();
    serde_json::from_slice(&bytes[boundary..]).unwrap()
}

pub async fn respond(socket: &mut TcpStream) {
    let first = json!({"id":"reply","object":"chat.completion.chunk","created":1,"model":"test","choices":[{"index":0,"delta":{"content":"done"},"finish_reason":null}]});
    let last = json!({"id":"reply","object":"chat.completion.chunk","created":1,"model":"test","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
    let body = format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n");
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}
