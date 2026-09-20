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

#![cfg(unix)]

use futures_util::{SinkExt, StreamExt};
use maka_config::{
    ConfigurationStore,
    access::{AccessCreateMode, AccessCredential, CredentialState},
};
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::access::ManagedPrincipalKind;
use maka_runtime_host::server::{Host, local::LocalListener, websocket::WebSocketListener};
use maka_transport::ndjson;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::tungstenite::{Error, Message, client::IntoClientRequest};
use tokio_util::sync::CancellationToken;

const SECRET: &str = "synthetic-remote-capacity-test-only";

fn hello(client: usize) -> Value {
    json!({"kind":"hello", "clientInstanceId":format!("capacity-{client}"),
        "protocolMin":0, "protocolMax":0, "compatibilityEpoch":maka_protocol::COMPATIBILITY_EPOCH,
        "compositionId":"maka.interactive"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_saturation_preserves_local_status_and_credential_revocation() {
    let directory = tempfile::Builder::new()
        .prefix("maka-capacity-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let namespaces = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let owner = Arc::new(RootOwner::create(&directory.path().join("root"), &namespaces).unwrap());
    let configuration = ConfigurationStore::for_root(owner.clone()).await.unwrap();
    configuration
        .create_access_credential(
            AccessCredential {
                credential_id: "capacity-credential".into(),
                credential_hash: format!("{:x}", Sha256::digest(SECRET.as_bytes())),
                principal_id: "capacity-owner".into(),
                principal_kind: ManagedPrincipalKind::RemoteOwner,
                grants: vec!["host.status".into()],
                can_publish_client_capabilities: false,
                can_use_host_paths: false,
                created_at: "2026-09-12T00:00:00Z".into(),
                state: CredentialState::Active {
                    client_instance_id: None,
                },
                capability_owner: None,
            },
            AccessCreateMode::Issue,
            None,
        )
        .await
        .unwrap();
    configuration.shutdown().await.unwrap();
    drop(configuration);
    let host = Host::open(Arc::try_unwrap(owner).ok().unwrap())
        .await
        .unwrap();
    let socket_path = directory.path().join("h.sock");
    let local = LocalListener::bind(&socket_path).unwrap();
    let websocket = WebSocketListener::bind("127.0.0.1:0".parse().unwrap(), vec![])
        .await
        .unwrap();
    let address = websocket.local_addr().unwrap();
    let cancellation = CancellationToken::new();
    let server = tokio::spawn(local.serve_with_websocket(websocket, host, cancellation.clone()));
    // Keep shutdown outside the assertion task so a regression also drains the
    // server and releases its private root instead of leaving detached work.
    let client_path = socket_path.clone();
    let scenario = tokio::spawn(async move {
        timeout(Duration::from_secs(20), async move {
            let request = || {
                let mut request = format!("ws://{address}/runtime-host")
                    .into_client_request()
                    .unwrap();
                request
                    .headers_mut()
                    .insert("Authorization", format!("Bearer {SECRET}").parse().unwrap());
                request
            };
            let mut clients = Vec::new();
            for index in 0..64 {
                let (mut client, _) = tokio_tungstenite::connect_async(request()).await.unwrap();
                client
                    .send(Message::text(hello(index).to_string()))
                    .await
                    .unwrap();
                let accepted: Value =
                    serde_json::from_str(client.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                assert_eq!(accepted["state"], "ready", "{accepted}");
                clients.push(client);
            }

            // TCP is connected before testing admission; all 64 occupied slots
            // are proven by protocol acknowledgements, not scheduling sleeps.
            let queued_socket = TcpStream::connect(address).await.unwrap();
            let queued = tokio_tungstenite::client_async(request(), queued_socket);
            tokio::pin!(queued);
            assert!(
                timeout(Duration::from_millis(200), &mut queued)
                    .await
                    .is_err(),
                "the 65th remote connection must wait for remote capacity"
            );

            timeout(Duration::from_secs(2), async {
                let socket = tokio::net::UnixStream::connect(&client_path).await.unwrap();
                let token = CancellationToken::new();
                let _cancel_on_exit = token.clone().drop_guard();
                let (mut reader, mut writer) = ndjson::split(socket, token);
                writer.write(&hello(64)).await.unwrap();
                assert_eq!(reader.read().await.unwrap().unwrap()["state"], "ready");
                writer
                    .write(&json!({"requestId":"status", "operation":"host.status", "input":{}}))
                    .await
                    .unwrap();
                let status = reader.read().await.unwrap().unwrap();
                assert_eq!(status["requestId"], "status");
                assert_eq!(status["result"]["state"], "ready", "{status}");
                writer
                    .write(
                        &json!({"requestId":"revoke", "operation":"access.credential.revoke",
                    "input":{"credentialId":"capacity-credential"}}),
                    )
                    .await
                    .unwrap();
                let revoked = reader.read().await.unwrap().unwrap();
                assert_eq!(revoked["requestId"], "revoke");
                assert_eq!(revoked["result"]["revoked"], true, "{revoked}");
            })
            .await
            .expect("local recovery must remain usable at remote capacity");

            timeout(Duration::from_secs(2), async {
                for mut client in clients {
                    loop {
                        match client.next().await {
                            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                            Some(Ok(_)) => continue,
                        }
                    }
                }
                match queued.await {
                    Err(Error::Http(response)) => assert_eq!(response.status(), 401),
                    other => panic!("queued upgrade must reject the revoked credential: {other:?}"),
                }
            })
            .await
            .expect("revocation must close remote sockets and release admission");
        })
        .await
        .expect("capacity scenario must complete");
    });
    let result = scenario.await;
    cancellation.cancel();
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!socket_path.exists());
    assert!(TcpStream::connect(address).await.is_err());
    result.unwrap();
}
