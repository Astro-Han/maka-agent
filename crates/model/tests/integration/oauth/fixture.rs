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

use super::*;
use maka_runtime::configuration::policy::{NetworkProxy, ProxyProtocol};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};

async fn head(stream: &mut (impl AsyncRead + Unpin)) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 16 * 1024);
        bytes.push(stream.read_u8().await.unwrap());
    }
    String::from_utf8(bytes).unwrap()
}
pub(super) async fn proxy(responses: Vec<Value>) -> (Client, tokio::task::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let policy = maka_network::Policy::from_settings(
        &NetworkProxy {
            enabled: true,
            protocol: ProxyProtocol::Http,
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            auth_enabled: true,
            username: "user".into(),
            bypass_list: vec![],
            auto_bypass_domains: vec![],
        },
        Some("secret"),
    )
    .unwrap();
    // Fixed provider URLs intentionally resolve inside this loopback-only CONNECT
    // fixture. Never change global CA trust or a production client.
    let client =
        Client::with_http_builder(policy.client_builder().danger_accept_invalid_certs(true))
            .unwrap();
    let server = tokio::spawn(async move {
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![
                    CertificateDer::from_pem_slice(include_bytes!(
                        "../../../../network/tests/fixtures/localhost.pem"
                    ))
                    .unwrap(),
                ],
                PrivateKeyDer::from_pem_slice(include_bytes!(
                    "../../../../network/tests/fixtures/localhost.key"
                ))
                .unwrap(),
            )
            .unwrap();
        let tls = TlsAcceptor::from(Arc::new(config));
        let mut requests = Vec::new();
        for response in responses {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let connect = head(&mut tcp).await.to_ascii_lowercase();
            let destination = connect
                .split_whitespace()
                .nth(1)
                .unwrap()
                .strip_suffix(":443")
                .unwrap();
            assert!(connect.contains("\r\nproxy-authorization: basic dxnlcjpzzwnyzxq=\r\n"));
            tcp.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            let mut stream = tls.accept(tcp).await.unwrap();
            let request = head(&mut stream).await;
            assert!(!request.to_ascii_lowercase().contains("proxy-authorization"));
            let mut headers = std::collections::BTreeMap::new();
            for line in request.lines().skip(1).filter(|l| l.contains(':')) {
                let (name, value) = line.split_once(':').unwrap();
                headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
            }
            let length: usize = headers
                .get("content-length")
                .map(|s| s.parse().unwrap())
                .unwrap_or(0);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            let raw = String::from_utf8(body).unwrap();
            let body = if headers
                .get("content-type")
                .is_some_and(|s| s == "application/json")
            {
                serde_json::from_str::<Value>(&raw).unwrap()
            } else {
                let url = reqwest::Url::parse(&format!("http://form.invalid/?{raw}")).unwrap();
                serde_json::to_value(
                    url.query_pairs()
                        .into_owned()
                        .collect::<std::collections::BTreeMap<_, _>>(),
                )
                .unwrap()
            };
            let path = request.split_whitespace().nth(1).unwrap();
            let auth_headers: std::collections::BTreeMap<_, _> = headers
                .iter()
                .filter(|(name, _)| {
                    matches!(
                        name.as_str(),
                        "authorization"
                            | "user-agent"
                            | "editor-version"
                            | "editor-plugin-version"
                            | "copilot-integration-id"
                            | "openai-intent"
                            | "x-github-api-version"
                    )
                })
                .collect();
            requests.push(json!({"url":format!("https://{destination}{path}"),"body":body,"contentType":headers.get("content-type"),"authHeaders":auth_headers}));
            respond(&mut stream, response).await;
        }
        requests
    });
    (client, server)
}
async fn respond(stream: &mut (impl AsyncWrite + Unpin), response: Value) {
    let raw = response
        .get("raw")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| response["payload"].to_string());
    let extra = response
        .get("headers")
        .and_then(Value::as_str)
        .unwrap_or("");
    stream.write_all(format!("HTTP/1.1 {} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{raw}",
        response["status"].as_u64().unwrap(),raw.len()).as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
}
pub(super) fn authorization(provider: Provider) -> Value {
    let payload = match provider {
        Provider::OpenaiCodex => {
            json!({"device_auth_id":"device +/","usercode":"M4KA","interval":"1","expires_at":"2030-01-01T00:00:00Z"})
        }
        Provider::XaiOauth => {
            json!({"device_code":"device +/","user_code":"M4KA","verification_uri_complete":"https://auth.x.ai/verify?user_code=M4KA","expires_in":60,"interval":1})
        }
        Provider::GithubCopilot => {
            json!({"device_code":"device +/","user_code":"M4KA","verification_uri":"https://github.com/login/device","expires_in":60,"interval":1})
        }
    };
    json!({"status":200,"payload":payload})
}
pub(super) async fn oracle(provider: Provider, responses: &[Value]) -> Value {
    oracle_input(json!({"provider":provider,"responses":responses})).await
}
pub(super) async fn refresh_oracle(
    provider: Provider,
    responses: &[Value],
    tokens: &Value,
) -> Value {
    oracle_input(json!({"provider":provider,"responses":responses,"previousTokens":tokens})).await
}
async fn oracle_input(input: Value) -> Value {
    use std::process::Stdio;
    let mut child = tokio::process::Command::new("node")
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/oauth-device-oracle.mjs"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .await
        .unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
