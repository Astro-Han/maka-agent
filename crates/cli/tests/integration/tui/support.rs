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
use serde_json::{Value, json};
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
    let login = login(
        &client,
        maka_protocol::oauth::Target::Create {
            provider: provider(&client, "openai-compatible").await,
            configuration: json!({"baseUrl":url}),
            slug: "tui-fixture".into(),
            name: "TUI fixture".into(),
        },
        "dummy-local-fixture",
    )
    .await;
    let created = client.request(Operation::ConnectionCatalogUpdate, json!({
        "expected":{"connectionId":login.connection.connection_id,"revision":1},
        "changes":{"name":"TUI fixture","configuration":{"baseUrl":url},"enabled":true,
            "enabledModelIds":["fixture-model"],"modelOverrides":{"fixture-model":{"contextWindow":128000}}}
    })).await.unwrap();
    let basis = &created["connection"];
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

pub(super) async fn provider(
    client: &maka_client::Client,
    name: &str,
) -> maka_protocol::model_provider::Identity {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let directory = client
                .provider_directory(maka_protocol::model_provider::Scope::Profile)
                .await
                .unwrap();
            if let Some(entry) = directory
                .entries
                .into_iter()
                .find(|entry| entry.identity.name == name)
            {
                return entry.identity;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture provider did not publish")
}

pub(super) async fn authenticate(client: &maka_client::Client, id: &Value, key: &str) {
    let (_, rows) = super::enabled_models::catalog(client).await;
    let row = rows
        .iter()
        .find(|row| row["kind"] == "connection" && row["connectionId"] == *id)
        .unwrap();
    let target = maka_protocol::oauth::Target::Existing {
        expected: serde_json::from_value(json!({
            "connectionId":id,"revision":row["revision"],"slug":row["slug"],
            "provider":row["provider"],"configuration":row["configuration"]
        }))
        .unwrap(),
        configuration: row["configuration"].clone(),
    };
    login(client, target, key).await;
}

async fn login(
    client: &maka_client::Client,
    target: maka_protocol::oauth::Target,
    key: &str,
) -> maka_protocol::oauth::LoginProjection {
    use maka_protocol::oauth::{LoginStart, Phase};
    let input = LoginStart {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        target,
        authentication: maka_runtime::provider::AuthenticationInput {
            method: "api-key".into(),
            input: json!({"apiKey":key}),
        },
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut result = client.start_oauth_login(&input).await.unwrap();
        while matches!(result.phase, Phase::Exchanging | Phase::Committing) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            result = client
                .query_oauth_login(&input.recovery(), Some(&result.connection))
                .await
                .unwrap();
        }
        assert_eq!(result.phase, Phase::Authenticated);
        result
    })
    .await
    .expect("fixture authentication did not settle")
}
