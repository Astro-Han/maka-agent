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
use maka_event_log::{
    EventLog,
    root::{ROOT_DATABASE, RootNamespaces, RootOwner},
};
use maka_runtime::access::ManagedPrincipalKind;
use maka_runtime_host::server::{Host, local::LocalListener, websocket::WebSocketListener};
use maka_transport::ndjson;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::tungstenite::{Error, Message, client::IntoClientRequest};
use tokio_util::sync::CancellationToken;

const SECRET: &str = "synthetic-pairing-expiry-test-only";
const OLD_SECRET: &str = "synthetic-already-expired-test-only";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn hello(client: &str) -> Value {
    json!({"kind":"hello", "clientInstanceId":client,
        "surface":"desktop", "activitySnapshotVersion":2,
        "protocolMin":0, "protocolMax":0, "compatibilityEpoch":maka_protocol::COMPATIBILITY_EPOCH,
        "compositionId":"maka.interactive"})
}

async fn credential_ids(root: &Path) -> Vec<String> {
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(root.join("configuration-rust.sqlite"))
            .read_only(true),
    )
    .await
    .unwrap();
    let documents: Vec<String> = sqlx::query_scalar("SELECT document FROM access_credentials")
        .fetch_all(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    documents
        .into_iter()
        .map(|document| {
            serde_json::from_str::<Value>(&document).unwrap()["credentialId"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_expiry_removes_durable_records_and_closes_idle_authenticated_peer() {
    let directory = tempfile::Builder::new()
        .prefix("maka-expiry-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let namespaces = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let root = directory.path().join("root");
    let owner = Arc::new(RootOwner::create(&root, &namespaces).unwrap());
    let configuration = ConfigurationStore::for_root(owner.clone()).await.unwrap();
    let expires_at = now_ms() + 5_000;
    for (id, secret, deadline) in [
        ("already-expired", OLD_SECRET, now_ms() - 1_000),
        ("live-pending", SECRET, expires_at),
    ] {
        configuration
            .create_access_credential(
                AccessCredential {
                    credential_id: id.into(),
                    credential_hash: format!("{:x}", Sha256::digest(secret.as_bytes())),
                    principal_id: format!("owner-{id}"),
                    principal_kind: ManagedPrincipalKind::RemoteOwner,
                    grants: vec!["host.status".into(), "access.credential.finalize".into()],
                    can_publish_client_capabilities: false,
                    can_use_host_paths: false,
                    created_at: "2026-09-12T00:00:00Z".into(),
                    state: CredentialState::Pending {
                        expires_at: deadline,
                        bind_client_instance: true,
                    },
                    capability_owner: None,
                },
                AccessCreateMode::Prepare,
                None,
            )
            .await
            .unwrap();
    }
    configuration.shutdown().await.unwrap();
    drop(configuration);
    assert_eq!(credential_ids(&root).await.len(), 2);
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
    let client_path = socket_path.clone();
    let client_root = root.clone();
    // Assertion failures and scenario timeouts still pass through server shutdown.
    let scenario = tokio::spawn(async move {
        timeout(Duration::from_secs(15), async move {
            let request = |secret: &str| {
                let mut request = format!("ws://{address}/runtime-host")
                    .into_client_request()
                    .unwrap();
                request
                    .headers_mut()
                    .insert("Authorization", format!("Bearer {secret}").parse().unwrap());
                request
            };
            let (mut peer, _) = tokio_tungstenite::connect_async(request(SECRET))
                .await
                .unwrap();
            peer.send(Message::text(hello("pending-client").to_string()))
                .await
                .unwrap();
            let accepted: Value =
                serde_json::from_str(peer.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(accepted["state"], "ready", "{accepted}");
            peer.send(Message::text(
                json!({"requestId":"pending-status",
                "operation":"host.status", "input":{}})
                .to_string(),
            ))
            .await
            .unwrap();
            let status: Value =
                serde_json::from_str(peer.next().await.unwrap().unwrap().to_text().unwrap())
                    .unwrap();
            assert_eq!(status["requestId"], "pending-status");
            assert_eq!(status["result"]["state"], "ready", "{status}");
            assert!(
                now_ms() < expires_at,
                "pending authority must work before expiry"
            );

            // Observe committed startup cleanup before the live deadline, without
            // invoking an access mutation that could incidentally trigger expiry.
            timeout(Duration::from_secs(2), async {
                loop {
                    if credential_ids(&client_root).await == ["live-pending"] {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("startup must durably remove already expired pairing");
            match tokio_tungstenite::connect_async(request(OLD_SECRET)).await {
                Err(Error::Http(response)) => assert_eq!(response.status(), 401),
                _ => panic!("already expired pairing must reject an upgrade"),
            }

            // Send no further peer requests: only the listener deadline can close it.
            loop {
                match peer.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => continue,
                }
            }
            assert!(
                now_ms() >= expires_at,
                "live peer closed before its deadline"
            );
            match tokio_tungstenite::connect_async(request(SECRET)).await {
                Err(Error::Http(response)) => assert_eq!(response.status(), 401),
                _ => panic!("expired pairing must reject reconnect"),
            }
            assert!(credential_ids(&client_root).await.is_empty());
            let socket = tokio::net::UnixStream::connect(&client_path).await.unwrap();
            let token = CancellationToken::new();
            let _cancel_on_exit = token.clone().drop_guard();
            let (mut reader, mut writer) = ndjson::split(socket, token);
            writer.write(&hello("local-owner")).await.unwrap();
            assert_eq!(reader.read().await.unwrap().unwrap()["state"], "ready");
            writer
                .write(&json!({"requestId":"local-status",
                "operation":"host.status", "input":{}}))
                .await
                .unwrap();
            let status = reader.read().await.unwrap().unwrap();
            assert_eq!(status["requestId"], "local-status");
            assert_eq!(status["result"]["state"], "ready", "{status}");
        })
        .await
        .expect("expiry scenario must complete within its deadline");
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

    let reopened =
        ConfigurationStore::for_root(Arc::new(RootOwner::open(&root, &namespaces).unwrap()))
            .await
            .unwrap();
    assert_eq!(
        reopened.next_access_credential_expiry().await.unwrap(),
        None
    );
    // Using time zero distinguishes record removal from expiry-time filtering.
    for secret in [SECRET, OLD_SECRET] {
        assert!(
            reopened
                .authenticate_access_credential(
                    format!("{:x}", Sha256::digest(secret.as_bytes())),
                    0,
                )
                .await
                .unwrap()
                .is_none()
        );
    }
    reopened.shutdown().await.unwrap();
    drop(reopened);
    assert!(credential_ids(&root).await.is_empty());
    let log = EventLog::open(&root.join(ROOT_DATABASE)).await.unwrap();
    assert!(
        log.prefix(1000, 1024 * 1024)
            .await
            .unwrap()
            .events
            .is_empty(),
        "pairing expiry and status must not create execution facts"
    );
    log.close().await.unwrap();
}
