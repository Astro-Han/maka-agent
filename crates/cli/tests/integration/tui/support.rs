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
use maka_protocol::Operation;
use serde_json::json;
use std::path::Path;

pub(super) async fn json_response(
    mut stream: tokio::net::TcpStream,
    status: &str,
    body: serde_json::Value,
) {
    use tokio::io::AsyncWriteExt;
    let body = body.to_string();
    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}

pub(super) async fn model_list_request(
    listener: &tokio::net::TcpListener,
) -> (tokio::net::TcpStream, String) {
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(20), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut bytes = Vec::new();
    loop {
        let mut buffer = [0; 2048];
        let n = tokio::time::timeout(Duration::from_secs(20), stream.read(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert!(n > 0 && bytes.len() < 16384);
        bytes.extend_from_slice(&buffer[..n]);
        if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let request = String::from_utf8(bytes).unwrap();
    assert!(request.starts_with("GET /v1/models "));
    (stream, request)
}

pub(super) async fn client(root: &Path) -> maka_client::Client {
    let discovery = maka_client::local::read_discovery(root).unwrap();
    let stream = maka_client::local::open_stream(&discovery.endpoint)
        .await
        .unwrap();
    let (client, mut notices) = maka_client::Client::connect(
        stream,
        &discovery.root_id,
        &discovery.host_epoch,
        maka_runtime_host::server::HostOperations,
    )
    .await
    .unwrap();
    tokio::spawn(async move { while notices.recv().await.is_some() {} });
    client
}

/// Configure an isolated Host's controlled model. The TUI itself uses Client::Operations.
pub(super) async fn model_client(root: &Path, url: &str) -> maka_client::Client {
    let client = client(root).await;
    let created = client.request(Operation::ConnectionCatalogCreate, json!({
        "expectedCatalogRevision":0,
        "connection":{"slug":"tui-fixture","name":"TUI fixture","providerType":"openai-compatible",
        "baseUrl":url,"enabled":true,"enabledModelIds":["fixture-model"],
        "modelOverrides":{"fixture-model":{"contextWindow":128000}}}
    })).await.unwrap();
    let basis = &created["connection"];
    client.request(Operation::CredentialVaultSet, json!({
        "locator":{"scope":"connection","connectionId":basis["connectionId"],"kind":"api_key"},"expected":null,
        "expectedConnection":{"connectionId":basis["connectionId"],"revision":basis["revision"],
            "slug":"tui-fixture","providerType":"openai-compatible","effectiveBaseUrl":url},
        "secret":"dummy-local-fixture"
    })).await.unwrap();
    client
        .request(
            Operation::ConnectionCatalogSetDefaultTarget,
            json!({
                "expectedCatalogRevision":created["catalogRevision"],
                "target":{"connectionId":basis["connectionId"],"modelId":"fixture-model"}
            }),
        )
        .await
        .unwrap();
    client
}
